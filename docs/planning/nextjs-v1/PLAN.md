# Next.js v1 enterprise product — implementation plan

Version: **N1-2026-09-23-R4**

제품·업무, 컴플라이언스·보안, 아키텍처·이관의 세 독립 검토자가 동일한 전체 계획을 모두 **ACCEPT**했습니다. 미해결 계획 이견은 없습니다.

승인된 정본 SHA-256: `8a59028e505f3c2d796414b23e2b0f907ecf0a33e6b977b6845b6f0e64f4e90f`

이 승인은 구현 계획에 대한 승인입니다. 구현 완료, 법적 인증, 성능·복구 시험 통과를 의미하지 않습니다. 현재 저장소 파일은 변경하지 않았으며, 쓰기가 허용되면 승인본과 요구사항·검토 기록을 먼저 저장한 뒤 구현합니다.

## 1. Product decisions and responsibility model

Deliver one Korean-first, object-native business platform for 3,000–10,000 registered people total. All business and platform capabilities below are launch requirements. Internal delivery waves are dependency ordering, not permission to omit later modules.

No AI/model features, fabricated business data, runtime seeds, simulated integrations, success fallbacks, empty feature shells, or required behavior replaced by a “not supported” banner. Deterministic computation and self-hosted OCR are included. Real failures, unavailable authority and deliberately disabled live operations remain visible.

Ordinary work begins with a native object, plan, task, conversation or business action—not a spreadsheet. Documents and sheets remain first-class collaboration and external import/export/processing tools. They never become a second payroll, permissions, inventory or accounting authority.

Reference catalogs, reviewed legal tables, UI templates and a real free entitlement definition are configuration, not fake business seeds.

Reuse verified Rust domain owners from Console in an independent frontend workspace. Do not modify, symlink to, rebase or operate the concurrently developed Console repository. Preserve v2 identity, contracts, domain ownership, receipts and full-state migration. OSS services may remain running through the v2 business-engine/UI cutover.

### People and context

These are responsibility profiles for design and testing, not hardcoded job-title roles. One person may hold several, with independent permissions for each Company/resource/action and effective interval.

| Profile | Primary jobs and landing experience | Authority/privacy boundary |
|---|---|---|
| Any worker, including own-factory and client-site staff | My work/shift, leave, payslips, benefits, announcements, messages, corrections and employment documents | Own current/historical entitlements; employment exit does not delete Account or lawful former-worker access |
| Planner, dispatcher, site or team operator | Demand, staffing, schedule, live work, exceptions, quality/safety and handover | Responsibility does not imply employer status, personnel authority or permission to issue client-side labor instructions |
| HR/recruiting/payroll practitioner | People and lifecycle, candidate work, actual-time reconciliation, wage calculation and employee response | Sensitive fields, approvals, payment and document delivery are separately authorized |
| Sales, procurement, finance or contract practitioner | Pipeline, obligations, purchasing, delivery acceptance, cost, AR/AP, ledgers and reconciliation | Commercial price, wage liability and cash movements are separate facts |
| Manager/executive or another authorized operator | Observe performance/risk, compare scenarios, allocate responsibility, decide and operate | Same scoped actions as practitioners; title alone grants nothing and never bypasses independent approval |
| Applicant, customer, supplier, partner or former worker | Purpose-specific portal, submissions, acceptance, disputes, shared documents and own rights | Explicit relationship and current resource/field authority; not mutually exclusive account classes |
| Domain/data/application builder | Define types, rules, pipelines, apps, functions and packages; simulate and publish | Can change definitions only within granted scope; cannot bypass domain owners or legal floors |
| Privacy/legal/audit/security/platform operator | Obligations, restricted review/export, access, incidents, capacity and recovery | Narrow audited duties; no universal administrator plaintext or business-write bypass |

Tenant = Group; standalone Company = Group of one. Company is the legal entity and PostgreSQL RLS cell.

Account, Person, Employment, CompanyActor, legal employer, intercompany contract, site ownership/operation, actual supervision, job, responsibility, authority and commercial/pay terms remain distinct. Employment and assignments use effective time and knowledge time.

A Group never arms a Company RLS context. Authorized consolidated reads fan out per Company and identify incomplete coverage without revealing unauthorized Companies. Changes remain Company-owned; no fictional Group-wide atomic undo.

### Navigation and experience contract

The fixed top-level navigation is:

| Destination | Contents |
|---|---|
| **Work — 업무** | Tasks, approvals, sales, contracts, procurement/finance, inventory/manufacturing, assets/field operations, facilities, rental, consulting and support workspaces |
| **People — 사람** | Organization, employment, recruiting, staffing, attendance, leave/benefits, training, employee relations, evaluation/surveys and directly visible **Payroll** |
| **Communications — 소통** | Mail, messenger, calendar and room polls |
| **Data — 데이터** | Documents/sheets, sources, pipelines, catalog, ontology, actions, applications, analysis/scenarios, code and packages |
| **Administration — 관리** | Access, identity proofing, integrations, tenant/billing, compliance/records, glossary and operations |

Contextual alternate entries open the same object, not duplicate modules. Payroll keeps its own approval progress even when reached from Work. Person, Employment, organization and payroll drill-through preserve a one-step return to the originating context.

Use a consistent shell with module-local Group/Company/site/department/self filters, current-context breadcrumbs, global authorized search, command palette, saved views, pinned objects and a contextual communication rail. Do not force a global Company switch for each task.

The landing view supports overview, discovery, planning, comparison and execution—not only exception queues.

Every named screen implements:

- Real authorized SSR loading.
- Truly empty/onboarding, populated and filtered-empty states.
- Refresh/staleness, validation failure and conflict comparison.
- Permission loss and partial-result handling.
- Pending, UNKNOWN, retry and terminal outcome.
- History, evidence and legitimate correction paths.

Every action displays its object, Company, actor capacity, effective date, consequences, inputs and next responsible person. Explain labels, units, examples and rules in plain Korean. No blank editor body, unexplained code, fake count or optimistic financial success.

Shared components include typed field/repeated-item editors; object/relationship selectors; source page/cell/region viewers; revision/diff and effective-date timelines; draft recovery; policy/impact inspectors; approval cards; task handover; status/receipt timelines; governed tables, boards, calendars, charts and maps; file version/preview/export; accessible dialogs and notifications.

Domain screens compose these with domain-specific calculations and workflows, rather than rendering every object as generic CRUD.

Use responsive 360/390, 768, 1280 and 1440px layouts, keyboard-only operation, Korean IME, screen-reader names, semantic tables/forms, visible focus, reduced motion and WCAG 2.2 AA. Large tables may scroll within a clearly labeled region; the entire page must not become an unusable horizontal sheet.

Incoming messages do not steal focus or scroll position. Server-saved drafts survive refresh/device changes. Private unsubmitted drafts do not become Company-visible automatically.

No persistent plaintext business data, decrypted messages or bearer tokens in browser storage; server cookies remain HttpOnly/Secure. The sole browser-data exception selected as an explicit planning default is an encrypted Matrix crypto store, with the unlock/erasure rules below. This is the recommended option posed to the user, not a claim of additional user approval.

No offline, native-app or PWA implementation.

## 2. Architecture, interfaces and operational invariants

### Chosen implementation

- **Frontend:** Next.js 16.3.5 App Router/SSR, React 19.3, Node 24 LTS, TypeScript, existing TanStack Query/Table, Zod, Tailwind and accessible primitives. Self-hosted. Remove static export/serve-out deployment. Zustand is UI state only. Reuse fitting components; replace existing visual structure where complete workflows require it.
- **Backend:** Existing Rust/Axum application/domain/adapter structure, Cedar 4.11.2 and PostgreSQL 18. Preserve domain names and owning use cases; converge REST routes that bypass application owners. One modular backend plus bounded worker roles, not a microservice per module.
- **Execution:** Reuse the PostgreSQL job/outbox/workflow runtime. Do not add Kafka, Temporal or a second workflow engine.
- **Canonical storage:** Transactional facts, commands, approvals, timers, outboxes, receipts, immutable file manifests and bounded encrypted BYTEA chunks use PostgreSQL. Native files use 4 MiB chunks and per-Company deduplication, not globally discoverable hashes.
- **Storage interoperability:** Retain the existing storage owner and S3 compatibility adapter for imports/migration; do not build a new S3 server. Stream uploads. Bound and isolate archive decompression, parsing and conversion.
- **Data plane:** Apache DataFusion/Arrow/Parquet in bounded workers; PostgreSQL owns dataset versions, lineage and publication. DataFusion is not a transaction authority.
- **Code execution:** SQL/typed expressions are the normal authoring path. Required Python/Rust code workbooks/functions use pinned, network-denied gVisor jobs, without host credentials, host mounts, browser eval or direct business-database writes. Dependencies are immutable, scanned and license-inventoried.
- **Search and visualization:** PostgreSQL FTS/pg_trgm and authorized structured filters for native/mail/archive search; PostgreSQL/PostGIS and standard chart/map components for geospatial/time-series views.
- **Identity:** Native passkeys remain the Account foundation. Implement real OIDC, SAML and SCIM/provisioning integration and conformance tests, initially disabled. Federation binds issuer + immutable subject and scoped Company identity, never mutable email. Preserve passkey RP IDs/origins and credential IDs through v2.
- **Anonymous ballots:** Apache-2.0 `@cloudflare/blindrsa-ts` 0.4.6, standard RFC 9474 RSABSSA-SHA384-PSS-Randomized with 3072-bit per-scope keys. Upstream documentation was checked; integration security review and RFC vectors remain release tests.

No paid search, hosted backend, model service or proprietary enterprise component is required.

Lock dependencies and sources with lockfiles/image digests. Verify compatible maintained patch releases when creating the implementation lock. Preserve researched major/version choices unless a security advisory requires a reviewed replacement.

Publish required OSS notices and corresponding modified AGPL source. Do not include Stalwart enterprise/SEL code or bypass licenses.

### Domain action and API contract

Reuse existing domain APIs behind their owners and preserve compatible aliases.

The common API surface is versioned under `/api/v1`:

- Action descriptors and drafts.
- Draft revision/edit/discard.
- Preflight and review requests/decisions.
- Commands and operation/receipt status.
- Authorized objects, search and history.
- Source/import/export jobs.
- Data/app/package definitions and runs.

Generated TypeScript SDK/OpenAPI types must be checked against actual path × method handlers and responses, not only generated schemas.

Canonical contracts include:

- `AccountId`, `PersonId`, `CompanyActorId`, `GroupId`, `CompanyId`.
- `ObjectRef(type/id/revision)`, `ActionType/version`, `Draft/revision`.
- `FieldAddress(property path/parent item IDs/current item ID)`.
- `SourceRef(original/version/hash/page-cell-region)`.
- `Approval(exact payload digest/natural-person identities/policy and authority versions)`.
- `OperationId/attempt`, `Receipt`, `EffectiveInterval`, `EvidenceVersion`.

Repeated items retain stable IDs while incomplete text is saved. Money/decimal/integer precision is exact on the server and encoded as strings where JSON numbers would lose precision.

Preserve canonical hash/codec versions. Use versioned typed canonical encoding; never silently rehash historical records.

Every surface follows the same owner path:

**registered action → versioned draft → exact preflight/review → owner transaction → durable receipt → independently reconciled external effect**

Authorization, input validation, period state, effective rules, expected revisions and resource invariants are rechecked at execution. A changed payload invalidates approval.

Human approval never means journal posted, institution accepted, mail delivered, wage paid or work completed.

Corrections initiate accountable work before a correction-effect receipt exists; only a claim of completed correction requires that result. Typed expected quantity/amount/unit claims do not grant source-write permission.

### Mandatory high-risk classification

The owner action registry enforces a non-weakenable semantic floor covering:

- Identity/authority overrides and privileged recovery.
- Binding employment or commercial-term changes.
- Monetary finalization, ledger posting and payment execution.
- Consequential institutional acts.
- Definition/package/policy publication affecting money, access, legal applicability or custody.
- Sensitive bulk disclosure, legal production, hold release and records destruction.

Aliases, imports, bulk commands and automation inherit their underlying action’s floor. Unknown classification fails closed. Tenant policy may strengthen, never downgrade it.

A harmless personal view is not automatically privileged. Directory/profile creation and private drafts are low-risk only when they do not grant legal authority or alter binding terms.

High-risk operations require the requester plus at least **one different natural-person approver**, including owner/admin/single-person-company cases. Different accounts or capacities of one human do not qualify.

Unverified human linkage fails closed for high-risk operations. Immediate onboarding creates a real self-declared tenant and low-risk private work; it does not certify representation or waive approval.

### Identity proofing and recovery

Implement:

**self-declared Account/Company → proofing request → attended evidence check → independently approved Person/account linkage and corporate-authority grant → active/expired/revoked/disputed status**

Screens include an evidence checklist, private appointment, verifier workbench, linkage conflicts, authority/effectivity and appeal/recovery history.

Initial v1 uses attended original photo-ID inspection, in person or an explicitly approved supervised operational session, plus a live passkey challenge. Inspect only lawful, necessary identity attributes. Do not retain RRN, full ID numbers, ID images, recordings or biometric templates.

Separately validate representation using official business/registry evidence and actual delegation or organizational resolution. Verified Company identity stewards may verify people within their evidence-backed delegated scope, not mint their own proof or representation.

Commission initial platform proofing staff and root Person records through a witnessed, recorded ceremony binding actual humans to passkeys and verifier scopes. Tenant ownership/self-registration is not this ceremony.

The requester plus one different authorized natural person is sufficient; no additional routine third approver is introduced.

Retain restricted proofing method/date, necessary matching attributes, source reference/result, verifier/subject/linkage/authority versions and expiry under a purpose-based retention schedule.

Compare existing verified Person records and previous names/accounts before linking. Ambiguous duplicates remain disputed and cannot approve high-risk work.

Recovery preserves Person identity, revokes replaced credentials and cannot create an independent second approver. Link merges/splits, verifier revocation and material authority correction invalidate affected unexecuted approvals and trigger review of prior effects.

Honest authorized verifiers and noncollusion are explicit trust assumptions. The system prevents self-minting/account tricks, not every forged document or colluding human.

### Authorization and session boundaries

Low-risk direct operations follow published policy without gratuitous approvals. Approval, assignment, responsibility, execution authority and payment rights remain separate.

Transfer/handover preserves original actors and evidence, rechecks recipient authority/capacity, and never transfers signatures or privileges. One responsible performer owns each independently completable task. Return-from-absence changes future responsibility under policy, not already-running work blindly.

Use HttpOnly/Secure server sessions, CSRF/Origin protection, short-lived scoped service credentials, passkey step-up, expiring/revocable grants and recovery ceremonies bound to nonce/time/new credential/new login.

Recovery atomically revokes old refresh families. Operators cannot accept terms for another person.

RLS runtime roles have neither ownership nor BYPASSRLS. Lists, details, links, counts, history metadata, SSR, search, export, realtime and downloads use current object/field/purpose policy. No unfiltered detail fallback.

Cache compiled policy bundles, not cross-request allow decisions.

### Durability and external effects

All live transactional/protocol reads route to the current primary. The physical standby is not an independently readable application endpoint.

Use synchronous `remote_apply` plus an explicit confirmation barrier:

1. Capture a post-commit WAL INSERT-LSN upper bound on the identified current primary.
2. Require authenticated primary flush and independent full-standby flush/replay at or beyond that LSN.
3. Verify the same system identifier, timeline and writer epoch.

A successful COMMIT call alone is insufficient: PostgreSQL can cancel synchronous waiting after local commit.

No confirmed UI/protocol acknowledgment, push, published file version or external execution may outrun this barrier. Locally visible unconfirmed records remain pending/UNKNOWN and cannot authorize downstream effects.

Each outbox claim also commits and passes the barrier before egress. A dropped response is queried by stable operation identity, not blindly retried.

Fence writers and external egress with a monotonic epoch and positive old-process/network fencing. Lease expiry alone does not prove an old sender stopped.

Official bank/filing/mail effects may remain uncertain. Remote acceptance followed by lost local status is not exactly-once delivery. Preserve attempts, receipts and UNKNOWN outcomes; reconcile before retry. Compensation is a new governed operation, not rewritten history.

Per-Company transactions can coordinate wider workflows but cannot make multiple institutions or Companies atomically succeed.

## 3. Complete business-module specification

Each row includes the universal screen/state contract above. Inputs supplement verified domain schemas; identifiers, Company, ownership, effective/knowledge time, revision, policy and evidence are common.

“Complete” means the business result and correction path are exercised against the real backend. Configuration and scenarios do not create fake live records.

| Module / responsible profiles | Screens and essential components | Native workflow, depth and acceptance |
|---|---|---|
| **B01 Organization, people and employment** / personnel steward, operator, worker | Legal-entity/site/org explorer, as-of chart, Person/Employment detail, positions/reporting, access review, offboarding/rehire | Capture employer, terms, job, site, actual supervisor and effective intervals separately. Support concurrent employments, future/retro changes, transfer/divestiture, unassigned staff and rehire. Reproduce as-of truth; revoke current authority without erasing Account, tenure evidence or former-worker publications. |
| **B02 Recruiting and workforce pool** / recruiter, staffing coordinator, applicant | Requisition/capacity board, candidate pipeline, qualifications/matching, interviews, offer/version/acceptance, rejection/return/retention | Requisition → sourcing/deduplication → evidence review → interview → offer → authenticated acceptance → employment/assignment. Deterministic matching reports complete scan status. Handle changed vacancy, repeat applicant, withdrawal/expiry and identity conflict. Accepted terms survive hiring; no automatic permanent rejected-candidate pool. |
| **B03 Contracts, staffing and sites** / contract operator, planner, site lead, worker | Contract-linked demand, allocation/calendar/map, legal applicability, qualification/conflict inspector, readiness/replacement | Model subcontract, dispatch and own-factory operations without a worker-type shortcut. Bind clauses, demand, employer, direction/acceptance, rates and hazards independently. Client requests follow legally appropriate paths; safety stop-work remains possible. Invalid plans never erase actual work. |
| **B04 Attendance** / worker, site confirmer, attendance/payroll practitioner | My shift/punch, roster, raw-event/break timeline, source comparison, corrections, period preview/close/reopen | Schedule → exact actual intervals → reconciliation → confirmation → close → retro correction. Handle overnight/KST boundaries, missing/duplicate events, actual breaks/waiting, holidays, denied GPS/device/network, unapproved overtime and paid periods. No fabricated breaks/down-rounding. Recompute paid and billable consequences separately. |
| **B05 Leave, benefits and welfare** / worker, approver, benefit operator | Entitlement ledger, calendar, requests/eligibility, accrual/promotion simulator, benefit catalog/claims/adjustments | Version statutory/contractual rules and accrual lots. Eligibility/coverage → decision → consumption → cancellation/retro correction. Include protected leave, changed employment/headcount, partial-day units, concurrent requests, failed promotion service and negative adjustments. Restrict medical details; no mandatory generic medical-reason collection. |
| **B06 Payroll, retirement and year-end** / preparer, independent approver, reconciler, worker | Population/coverage, source readiness, calculation/rule trace, comparison, approval, payslip/publication, transfer reconciliation, retro/year-end/severance | Include proration, hourly/monthly work, overtime/night/holiday, absence/leave, multiple employment, ordinary/average/minimum bases, separate tax/insurance bases/caps/rounding, deductions/refunds and DB/DC/IRP/severance. Distinguish liability, approval, cash submission/partial/unknown/settled, journals and document delivery. Account for every target person and amount. Payslip timing does not wait blindly for bank success. |
| **B07 Performance, feedback and engagement** / subject, evaluator, coordinator | Cycle/rubric/goals, authenticated self-review, evidence/calibration, response/appeal, restricted history | Criteria → evidence → actual subject self-review → evaluation → calibration → notice/response → finalization/correction. Handle changed evaluator, missing review and concurrent employment. Bind rubric/result versions; explain deterministic adverse decisions. No model scoring, impersonated self-review or overwritten assessment. |
| **B08 Work hub, calendar and notifications** / any responsible person | Overview/planner, task board/detail, recurring series, personal/team/resource calendar, RSVP, notes, announcements/receipts/preferences | Task/dependency → schedule → evidence → close/reopen. Preserve series identity and occurrence exceptions. Handover covers absence, overload and instruction. Resolve resource/timezone/series conflicts. Notification failure differs from legal service. No video-conferencing or transcription service is added. |
| **B09 Approvals and electronic decisions** / requester, independent approver, executor, auditor | Draft, impact/review, inbox, assent/signature intent, execution/reconciliation, amendment/reversal | Exact artifact/version/authority → independent decision → current-policy execution → receipt → reconciliation. Cover changed payload, two accounts, revoked delegation, route/quorum/conflict, rejection/expiry and partial effects. Authentication is not document assent. Correction work may begin before its future result exists. |
| **B10 CRM and revenue planning** / account owner, sales practitioner, successor | Accounts/contacts, opportunity board, activity/proposal, forecast/renewal comparison, outcome correction | Evidence-based opportunity → proposal → won/lost → contract → delivery → renewal. Record relationship/purpose, owner, stage facts, amount/date and renewal series. Deduplicate, hand over leavers and reopen mistakes. Forecasts disclose assumptions and missing data. |
| **B11 Contracts, tenders and grants** / steward, performer, counterparty, finance | Clause/version comparison, rates/obligations, tender/grant checklist, assent, obligation calendar, variations/disputes/renewal | Monthly/headcount/time/output/mixed formulas, acceptance criteria, caps, indexation, SLA, tax and effective changes. Review/assent → obligations → actual acceptance → variation/dispute → close/renew/recovery. Trace staffing/pay/bill results to clauses. Internal checkboxes are not counterparty evidence. |
| **B12 Procurement, finance and accounting** / requester, receiver, bookkeeper, approver, reconciler | Budgets/commitments, requisition/PO, receiving/matching, invoices/credits, AR/AP, journals/trial balance, bank reconciliation, close | Need → PO → partial receipt → invoice match → liability/posting → payment/collection → reconciliation → adjustment. Include returns, duplicate invoice, FX/tax, partial settlement and locked periods. Ledgers balance and remain immutable; close races serialize. Do not blend legal ledgers or double-count dimensions. |
| **B13 Inventory and warehouse** / receiver, planner, picker, counter | Item/unit/lot/bin, putaway, availability/reservation, replenishment, pick/pack/ship, holds, count/discrepancy | Receive → inspect/quarantine → putaway → reserve → ship → return/count adjustment. Preserve conversions, serial/lot/expiry genealogy and partial movements. Serialize contested quantities, split/merge and reservations. Imports cannot create negative stock or defeat holds. Balances reconcile to movements. |
| **B14 Manufacturing/MES** / planner, operator, quality lead | BOM/routing/order, resource dispatch, operation capture, materials/genealogy, quality, Andon/OEE/cost | Demand → reserve → release → operations/consumption → inspection → finished lot → cost/acceptance. Include scrap, rework, substitution, split/merge, downtime and recall. Conserve materials/labor/output; expose actual OEE denominators. |
| **B15 Assets, equipment and maintenance** / custodian, technician, planner | Asset/custody/site, history, preventive series, work-order planner, parts/downtime/cost, disposal | Acquire → commission → assign → inspect/maintain → return/retire. Track condition, safety dates, custody, parts, labor and evidence. Handle loss, missing parts, overlapping downtime and canceled occurrences. Unsafe assets cannot be allocated; approval is not completion or collection. |
| **B16 Field service, logistics and dispatch** / coordinator, field worker, verifier | Capacity/vehicle/resource plan, routes/consignments, responsive job pack, evidence, POD/acceptance, disputes/cost | Request → suitable allocation/load → travel/start → perform/handoff → partial acceptance/dispute → payable/billable outputs. Handle wrong site, reassignment, safety refusal, failed delivery, return/damage, clock/network failures. Label unsent input honestly; preserve confirmed work. Show solver/route/capacity limits. |
| **B17 Compliance, safety, privacy and audit** / accountable operator, specialist, auditor | Applicability/obligations, rule impact, inspections/incidents, rights requests, retention/holds/erasure, audit/legal production | Official source + actual facts → prevent/detect → assign/remediate → independent verification → closure → retention/deletion. Handle conflicting/future rules, actual dispatch, accidents/stop-work, rights/hold conflicts and misuse. Show evidence/reasons/uncertainty, not a legal-pass score. |
| **B18 Support and knowledge** / requester, service owner, specialist | Portal/inbox, tickets, assignment/SLA calendars, knowledge, escalation/problem/reopen | Intake/email correlation → classification → one owner → investigation → resolution → response → close/reopen/problem. Version timers/calendars and deduplicate escalations. Contract SLA differs from platform SLO. Support access is scoped/time-limited/audited; a reply alone does not resolve an obligation. |
| **B19 Documents, evidence and office** / author, verifier, recipient, records steward | Library/search, editor/version/compare, quarantine, extraction/mapping, connected workbook, export/delivery | External mail/file → original → scan/parse/OCR → typed staging → repair → governed native action → interoperable response. Preserve page/cell/region, hidden-content classification, units/locale, formulas and original/derived linkage. Handle encryption, malware, malformed formats, edit conflicts and partial conversion. |
| **B20 Mail, messenger, polls and collaboration** / sender, recipient, delegate, archive custodian | Mailbox/thread/compose/queue/bounce, E2EE rooms/DMs/search, device/recovery, calendar links, polls, archive/legal export | Hosted mail and E2EE work messages with attachments, edits/redactions and offboarding-safe history. Explicit poll electorate/options/closure/duplicate prevention. Anonymous responses require unlinkable permits and privacy-safe publication, not merely hidden names. |
| **B21 Integrated facilities management** / operator, technician, customer verifier | Facility/service/SLA overview, obligation plan, triage, safe scheduling, work evidence, acceptance, energy/cost | Obligation → triage → hazards/readiness → schedule → service/inspection → customer acceptance/dispute → SLA/cost. Track actual units/readings/denominators. Missed service creates recovery work. No fabricated savings or self-certified customer acceptance. |
| **B22 Rental and lifecycle commerce** / allocator, rental operator, custodian, finance | Availability/suitability, quote/contract, reservation, handover/condition, return, refurbishment/resale/redeployment | Asset → quote/contract → exclusive reservation → handover → service → return/dispute → charges → repair/redeploy/resale. Preserve custody, condition and quantity/value accounting. Handle overlaps, unsafe assets, late/lost return and disputed damage. |
| **B23 Consulting and improvement** / consultant, client owner, performer | Diagnostic/project workspace, findings/evidence, initiative comparison, execution plan, benefit/sustainment | Diagnose → review findings → select initiative → execute → verify measured result → sustain/correct. Bind baseline, grain, assumptions, deliverables and customer acceptance. Missing baseline or changed conditions cannot become claimed savings. |

### Required People/compliance leaf workflows

A generic case or ticket does not satisfy these workflows.

| Leaf / responsible profiles | Screens and native lifecycle | Failure/correction and terminal evidence |
|---|---|---|
| **B05a Training and qualifications** / worker, instructor, steward, planner | Course/requirement/recurrence catalog, enrollment, sessions/attendance, assessment, verified completion, certificate/expiry/renewal, transcript | Completion updates effective qualification evidence and actual assignment eligibility. Verify issuer/identity; handle failed, expired, withdrawn or partial completion. Attendance is not a passed assessment. Corrections invalidate affected future allocation and open repair work. |
| **B17a Independent reporting** / reporter, protected case handler, independent identity custodians | Anonymous/sealed-identity intake, safe receipt/access secret, independent triage/conflict routing, investigation/evidence, identity-opening ceremony, remedy/closure/control improvement | Bypass ordinary line management. No identity/IP logs for anonymous intake. Named identity is separately sealed; opening requires requesting custodian plus another authorized custodian and step-up. Ninety-day protective monitoring is an operating default, not a statutory limit. Relevant adverse personnel actions enter independent protection review. |
| **B17b Grievance, discipline and appeal** / subject, investigator, decision maker, reviewer | Confidential intake, allegations/evidence, hearing/response, recusal, reasoned decision/notice, appeal, remedy/closure | Preserve real response opportunities and exact notices. Reversal creates corrective employment/payroll work while preserving evidence. Apply actual harassment, dismissal/protected-period and work-rule requirements, not prototype legal shortcuts. |
| **B17c Injury and compensation** / worker, safety/HR, claims practitioner | Protection/incident link, treatment/absence, official claims/status, compensation/payroll coordination, accommodation/return-to-work | Employer refusal cannot erase claim rights. Restrict medical fields. Injury absence is not silently annual leave or misconduct. Derive liabilities, protected periods and offsets from current law. Separate rejection/appeal, partial/unknown cash, return restrictions and settlement. |
| **B17d Rules of employment** / steward, worker representative, publisher | Clause/version comparison, adverse-impact analysis, consultation/consent, filing/publication/service, effective policy links, challenge/correction | Apply actual headcount, representation and adverse-change requirements. Receipt is not consent; filing does not cure invalid adoption. Preserve effective versions, reconcile linked policies/calculations and repair orphan rules. |
| **B04a Location and attendance choice** / worker, confirmer, privacy steward | Purpose-specific notice/permission, on-demand check-in, withdrawal/history, manual evidence/confirmation | Refusal, withdrawal or unavailable GPS never blocks truthful time reporting. No continuous tracking. Default to necessary check-in assessment/evidence, not raw coordinates. Manual confirmation uses the same attendance owner. Browser permission is not lawful basis. |

Required cross-module journeys:

- Contract → staffing → actual work → customer acceptance/dispute → payroll and billing → GL/bank/AR/AP → correction.
- Purchase → receipt → inventory → production/field consumption → cost.
- Asset → rental → maintenance → return/redeploy.
- Source → quality repair → governed data/object → scenario → reviewed operation.
- Worker/applicant/partner → current or historical self-service.
- Signup → real free work → entitlements/invoice without live collection.

## 4. Platform capabilities and selected OSS integrations

Builders configure capabilities inside the same governed object engine. Publishing a definition never grants its author runtime access to the underlying business data.

| Platform module / responsible profiles | Required screens/components | Implemented workflow and acceptance |
|---|---|---|
| **P01 Sources and connections** / integration steward, mandated operator | Discovery, mandate/credentials, capability tests, schemas, cursor/sync/health, rotation/revocation | Authorized source → scoped connection → actual test/read → schema version → incremental acquisition → reconciliation/repair. Separate read/sign/submit/notice capability, expiry, partial pagination, drift/backoff and stopped credentials. A saved URL is not “connected.” |
| **P02 Ingest, mapping and quality** / data operator, domain verifier | Inbox, scan/OCR/source viewer, typed mapping, quality, quarantine/repair, publish/replay | Preserve originals/offsets; parse → validate → account for every item → repair → reviewed dataset or owner-action publication. Separately count failed, duplicate, explicitly ignored and accepted items. Preview/execution share versions; retries do not duplicate effects. |
| **P03 Pipelines** / builder, operator | DAG/SQL, branch/diff, tests/preview, schedules, runs/checkpoints/backfill/cancel/retry | Version → isolated test → publish → trigger → checkpointed run → atomic publication. Handle late/out-of-order inputs, partial failures and superseded runs. Never expose half a dataset or rerun committed actions blindly. |
| **P04 Data products/catalog** / owner, consumer, steward | Catalog/search, schema/profile, row/column/source lineage, freshness/impact, access, deprecation | Publish immutable versions with ownership, meaning, grain and quality evidence. Propagate source restrictions; wider access requires reviewed declassification. Lineage metadata cannot reveal deleted/inaccessible secrets. |
| **P05 Ontology** / builder, consumer | Types/objects/sets/views/links, shared interfaces/properties, cardinality/derived fields, branches/migration/as-of | Model → validate → preview dependencies → migrate/publish → operate → correct/retire. Preserve typed references, link evidence, history and action compatibility. No silent reinterpretation of money or authority. |
| **P06 Actions, functions and forms** / builder, user, approver | Typed form/action/criteria editor, functions, simulation, review/invocation, Action Log/reversal | Define descriptor pointing to owner → validate/test → publish → draft/review/execute → receipt. Same invariants for UI/API/import/job/bulk. Arbitrary code cannot directly write approved tables. |
| **P07 Policies, calculations and automation** / steward, operator | Rules, legal source/effectivity, applicability, tests/traces, review/publication/rollback, trigger/run inspector | Event/time/object-set → bounded complete population → versioned rule → proposal or authorized action → accountable outcome. Prevent partial scans, loops and retroactive law rewrites. Legal floors override invalid customer configuration. |
| **P08 No-code applications** / builder, end user | Pages/navigation/widgets/queries/actions, variables/events, responsive/persona preview, draft/test/publish/upgrade/retire | Author governed screens → test scopes/states → publish/run → upgrade/rollback. No unrestricted HTML/JS or privileged query escape. App visibility is not data permission. Existing installations undergo real upgrades. |
| **P09 Analytics/reporting** / analyst, practitioner, observer | Object/table/time/geospatial explorer, metrics/grain, charts/pivots/dashboard, drillthrough, reports/distribution/API | Governed source → explicit metrics → verified drillthrough → publication/export with current recipient authority. Show denominators, coverage, freshness and lineage. Distinguish actuals, forecasts and scenarios; prevent aggregate/export leakage. |
| **P10 Scenarios/optimization** / planner, decision maker | Frozen baseline, assumptions/deltas, constraints/objectives, comparison/map/timeline, solver status, publication | Freeze → alternatives → deterministic solve → feasible/infeasible/timeout evidence → select → current-state review → owner actions. No live scenario writes or unexplained best-score recommendation. |
| **P11 Tenant/storefront/billing** / account holder, tenant operator, billing practitioner | Signup, Group/Company/members, plan/usage, entitlements, invoices/credits, bank instructions/reconciliation | Immediate free signup → native work → measured entitlements → terms → invoice/transfer record → independent reconciliation. Published prices only. Live collection initially disabled. |
| **P12 Runtime workspace** / every user | Home, navigation/search/pins, object panels/comms, views, drafts/recent work, help/preferences | Resume private work, discover and compare authorized objects, operate across permitted contexts without losing drafts or switching every tab. Revalidate deep links; counts/suggestions cannot leak objects. |
| **P13 Developer projects/workbooks** / builder | Projects/branches, SQL/Python/Rust, dependency locks, tests/logs/results, review/publication, API apps/SDK | Immutable source/runtime → isolated authorized test → publish → bounded execution → lineage/results/proposal. Enforce sandbox, resource/output limits and revoked-token cancellation. |
| **P14 Packages/installation fleet** / publisher, installer, operator | Bundles/dependencies, signatures/hashes/diffs, compatibility/migration, cohorts, customization/rollback | Lock dependencies → test clean install/populated upgrade → approve cohort → migrate/verify → advance or compatible rollback. Preserve customizations explicitly. Implement existing-tenant triggers, not merely callable installers. |
| **P15 Operations/resource governance** / platform/data operator, accountable owner | Service/source/job health, fairness/quotas, incidents/runbooks, audit/rotation, capacity/restore proof | Detect → identify affected Company/work/population → assign repair → reconcile/retry → verify/close. Bound global/per-Company work. Distinguish queued/stalled/failed/unknown; preserve operation identity. |

### Workflow efficiency and glossary

Deliver user-facing workflow-efficiency measurements, not only infrastructure metrics.

Authorized workflow owners see actual completion/abandonment, interaction counts, transition latency and failure/recovery funnels by workflow/version and permitted cohort. No fabricated historical counters or individual worker-performance scoring.

Collect minimal action type/state/duration/count telemetry, not field values, documents, identity-rich traces or anonymous-ballot interactions. Separate operational correlation from privacy-protected published aggregates. Apply retention, small-cohort suppression and access policies.

The governed glossary screen provides Korean/English term search, domain filters, current/source legal citations, explanations/examples and language preview.

Terms/translations have draft/review/publication/effective history, impact checking and correction. Published glossary entries feed labels and contextual help. Glossary publication cannot amend a legal rule or make an unreviewed future statute effective.

Prove label propagation, authority denial, citation versioning and measurements from real backend journeys.

### Definition, instance and installed-package lifecycle

Schema, action/rule, app/package and business-instance lifecycles are distinct and versioned.

Unpublished definitions execute only in isolated simulations against authorized non-operative inputs. They cannot create production instances or emit external effects. Draft edit/discard are real operations.

Published definitions are immutable. Changes create reviewed versions. Current legal and authorization floors apply even when invoking an older definition.

Instance creation is an explicit maintained action contract, separate from editing. Validate identity/property types, requiredness, compatible links/cardinality and referenced published versions together.

Each instance retains its schema version. Adding a required field never fabricates defaults or hides existing rows. Existing instances stay visible and editable through their compatible published schema and current policy until a reviewed backfill supplies valid values.

An incompatible upgrade activates for a cohort only after every selected instance and dependency reconciles; unresolved items remain counted. Package manifests pin schema/action/rule/app dependencies. Installed upgrades are real scheduled/cohort work.

Retirement stops new creation while supporting historical read, authorized correction/export and compatible outstanding work. Definition changes never remove retained records.

Rollback restores compatible routing and carries forward accepted data; it never rewinds committed business facts.

Test draft isolation, create-without-edit, populated required-field upgrade, partial backfill, concurrent edits and retirement with pending work through real browser/API paths.

### Data-plane ownership

Use immutable version manifests and Parquet content, metadata branches/deltas, atomic publication, durable append-stream checkpoints, ordering keys and late-event policies. Schema evolution is versioned.

Dataset publication never overwrites approved native facts. Source-derived proposals and authorized business overrides remain separately visible; native changes go through responsible actions.

### Office, connected sheets and external formats

Select **Euro-Office v9.3.4-hotfix.1**, reviewed source `abfa2946fcc7b6f31df497f73d7e48ca6dbef16b`, for self-hosted collaborative DOCX/XLSX/PPTX.

Record AGPL/asset notices and supply modified corresponding source. Configured connection ceilings are not measured capacity.

Collabora CODE is not its vendor’s recommended production edition. Paid editions and ONLYOFFICE Community’s unsuitable deployment limits do not satisfy this operating model.

Learn/reuse fitting openly licensed components and patterns from Nextcloud repositories, not a second competing whole groupware/identity platform.

Documents, slides and sheets support create/open/coedit/comment/review/version/restore/compare/export and authorized external exchange.

Excel acceptance includes multiple sheets, formulas, references, pivots, charts, formatting and round-trip golden files. Warn about significant unsupported content before destructive export. Preserve macro-bearing originals; **never execute VBA**.

A conversion that loses significant content is non-authoritative, not silently successful.

Connected sheets enforce six visible cell roles:

| Cell role | Behavior |
|---|---|
| Linked source | Read-only projection; inspect and edit through the source’s authorized workflow |
| Business input | Editable durable proposal, not committed business truth |
| Calculated result | Domain-owner calculation with trace/inspection; no manual overwrite |
| Sheet formula | Sandboxed analytical computation, never authority |
| Analytical input | Saved assumption/note; becomes business data only by explicit governed promotion |
| Object reference | Actual authorized object selection with ambiguity resolution |

Local formulas have no credentials, network or arbitrary code execution. Their displayed results never replace owner calculations.

Arrow/Tab/Enter, select/type/double-click, Escape and the formula bar respect role and Korean IME. Rectangular TSV paste/fill processes each typed cell, normalizes unambiguous money/units, retains valid proposals alongside actionable errors and never overwrites protected columns.

Sorting changes presentation, not identity or rank. Stable row/item/cell identities and selection survive drill-through.

Commitment shows affected population and relevant domain consequences, including gross/deduction/net variance where applicable, then uses owner review/execution.

Connected sheets include governed dataset ranges; manual/automatic/snapshot refresh; visible source/freshness; protected typed columns; range-to-dataset publication; and action-backed writeback. Writeback has row/item identity, conflict handling and receipts—not arbitrary SQL updates.

Patch Euro persistence to provide:

- Atomic replacement instead of delete-then-insert.
- Propagated PostgreSQL chunk-write errors.
- Current document-writer epoch.
- Idempotent scoped callbacks.
- Two-site durability before change/save acknowledgment.

The PostgreSQL change log remains canonical until the immutable file version is durably published. Caches/conversion files are disposable. A callback cannot publish a version before content durability.

Isolate editor origin and use short-lived document/Company sessions. Revalidate revocation, export and download authority. Test concurrent save, replacement/chunk failure, expired access, reconnect and crash reconstruction.

For HWP/HWPX, use isolated LibreOffice plus **H2Orestart v0.7.14** for validated viewing/PDF/ODT/DOCX conversion. Preserve originals, runtime/version and source relationships. Test significant text, tables, fonts, layout and pages.

No universal lossless-conversion claim or native collaborative HWP authoring/HWP export promise.

Scanners/OCR/converters have neither business credentials nor outbound network. Do not log raw document bytes. Extracted fields remain proposals with visible uncertainty and source regions.

### Hosted mail, contacts and calendars

Select **Stalwart v0.16.23**, source `9d1c75ab68435e4417337f768291e5f947686203`.

Build with `--no-default-features --features postgres`. Do not inherit upstream defaults that include unselected components or enterprise/SEL code.

Use reviewed PostgreSQL data/blob/search/memory stores. Whole MIME is BYTEA with a **40 MiB** admitted message limit.

Implement a bounded OSS isolation patch at shared principal/access-token/resource/ACL/directory/search/send owners. A Console BFF alone is insufficient because IMAP/JMAP/SMTP/CalDAV/CardDAV are direct protocols and upstream account/email resolution is global.

Every cross-principal lookup/delegation enforces current Company membership and explicit grants. Use an explicit permission allowlist, no Impersonate or alternate self-managed credential bypass, private administration and Console-issued revocable protocol/Company-scoped app tokens.

Do not rely on email-only OIDC mapping or unsafe default audiences.

Ship hosted MX/submission, TLS IMAP/JMAP, domains/aliases, personal/shared/delegated mailboxes, sender identities, folders/search/rules, queues/bounces/DSNs/retries, quotas, lifecycle/export/import, contacts and calendar invitations. POP3 is not required.

Institution-notice bodies cannot escape through an uncontrolled generic mailbox.

SMTP `250` requires durable MIME, envelope/queue/archive metadata and the two-site barrier. Patch/disable silent spam/Sieve discard and report paths that acknowledge without durable admission; use explicit rejection or durable quarantine.

Inbound/local delivery/egress/DSNs and protocol reads obey current epoch and confirmation rules. Lost remote acceptance remains UNKNOWN and may be duplicate-prone; do not promise exactly-once Internet mail.

Before public exposure, prove isolation/revocation, no open relay, abuse/signup/rate limits, domain control, DNS/PTR/ports and actual deliverability.

Configure SPF/DKIM/DMARC, TLS, MTA-STS/TLS-RPT, certificate/key rotation and restore. Public ports/IP do not prove reputation or deliverability.

### E2EE work communications and records custody

Select **Synapse v1.161.0**, Matrix Rust SDK commit `16f2683c89f837c9be7500d76c79a7c77e860f83` and JS SDK commit `bd6f8148428842135cb1bb873bd343c083032a1e`.

Use standard Matrix crypto; no custom encryption capsules or cryptographic fork.

Work DMs/rooms are E2EE with a **disclosed Company archive as a trusted participating endpoint**. Do not claim the Company cannot decrypt.

Each Company archive has one account and two distinct verified devices, one per independent full data site. Use standard encrypted SQLite crypto stores, independent to-device sync and bounded multiaccount worker processes—not a deployment per tenant.

Preserve full crypto stores, cross-signing, sessions and keys across backup/migration. Room-key exports alone are insufficient.

Serialize archive-device SDK store transitions. Under the store lock, use SQLite’s online-backup API, including WAL effects, to capture an immutable encrypted generation in canonical PostgreSQL storage.

Content, generation manifest, device/epoch identity and recoverable wrapping-key version pass the two-site barrier **before readiness, acknowledged crypto transitions or outgoing crypto requests**.

Use unchanged-chunk reuse and bounded batching without letting acknowledgments outrun the snapshot. Maintain encrypted wrapping-key recovery envelopes on both full data sites under separate recovery custody. Test real unwrap/restore.

Fence old device instances before restoring the newest accepted generation. Never roll back an accepted generation or reuse it from a stale process.

#### Media-upload durability

Use a synchronous Synapse storage provider backed by the existing PostgreSQL chunk owner.

Immutable MXC bytes, ownership, hash and Synapse metadata pass the confirmation barrier **before upload `200/content_uri`**, even before any message references the upload.

Local media/thumbnail files are disposable caches. Orphan uploads have disclosed bounded staging retention and hold-aware cleanup. An acknowledged upload remains recoverable during that retention.

Test site loss after upload acknowledgment but before message admission.

#### Browser device continuity

Use SDK `storageKey`-encrypted IndexedDB; upstream defaults are not encrypted.

One exclusive SDK client owns each store through Web Locks; same-session tabs coordinate through BroadcastChannel.

The storage key is a random 32-byte device key, never persisted plaintext. Wrap it using an HKDF-derived WebAuthn PRF key with AEAD binding to Account, device, RP, credential and envelope version. Keep only the ciphertext envelope server-side.

For passkeys without PRF, use a user-held high-entropy recovery key—not a low-entropy password or hidden server plaintext key.

Refresh unlocks the same device store; normal navigation retains the key in memory. Do not persist decrypted timelines, attachments or access tokens.

Registration/verification/recovery uses standard SDK cross-signing, secret storage and key-backup flows, current Account proof and explicit device trust. Never auto-trust by email.

Logout/account switch purges local store, keys and session data. Revocation stops future access and key sharing.

Another device recovers through an existing verified device or recovery key plus current passkey/identity recovery.

If all client secrets are lost, disclose it: govern establishment of a new crypto identity, retire old devices and recover eligible retained work history from the independent Company archive. Do not pretend lost private keys were recovered.

Standard SDK sharing may restore eligible retained room sessions to a new verified device. Already delivered recipient plaintext/keys cannot be remotely erased or made unknown again.

#### Message admission

The sender SDK shares room keys before encryption/send through the independent to-device path.

Synapse forms the real EventBase/event ID before its stable spam callback. The hook requires both archive devices to:

1. Decrypt the exact candidate through the standard SDK.
2. Validate schema and controlled local MXC attachment hashes/keys.
3. Durably persist prepared archive content and attachment evidence.

Missing key/device/replica causes bounded waiting, then truthful pending/failure. Never wait for that same room event through room sync.

Add a narrow final persistence guard covering normal clients, appservices, modules, admin/internal batches, room creation, membership/state/encryption/power changes and redactions.

Atomically bind operation identity to committed event and current Company/room/key epoch. Disable federation, public registration, raw-admin bypass and fake-200 shadowban IDs. Edge route blocking alone is insufficient.

An operation binds canonical decrypted payload, target and attachments; each attempt binds ciphertext/session/candidate. Retrying a committed operation resolves its original event. Timeout is not proof of noncommit.

Re-encryption requires authoritative noncommit reconciliation; a changed payload conflicts. Preserve SDK replay checks. Prepared archive data is not displayed as sent and is cleaned only after safe resolution.

Serialize membership/power/history/encryption/archive-device changes with admissions. Revoke future access and rotate future sessions.

Edits are new operations. Redaction/hiding differs from retention/legal hold. Archive/search/export uses current Company, purpose, field and records authority with audit and disclosure—not unrestricted platform-admin plaintext access.

### Room polls and anonymous HR surveys

Room polls and HR engagement surveys are separate experiences.

Named room polls provide scoped electorate/options, voting/closure and accountable results. Anonymous polls/surveys share the protected ballot owner, not attributable response rows with hidden names.

**B07a HR surveys** provide questionnaire/version/period/electorate setup, participant invitation and typed Likert response, collection/reminders, closure, privacy-safe organizational exploration, retention and destruction.

Freeze roster-derived organizational axes and coarsen small groups before issuance. Respondents cannot self-declare false/revealing cohorts. One active anonymous HR survey per Company.

No anonymous free text or post-submission editing/withdrawal that creates identity-to-ballot linkage. Explain this before submission. Handle applicable rights through the privacy process without inventing a reverse mapping.

Use a distinct RFC 9474 signing key for each Company, poll/survey, frozen definition and disjoint coarsened cohort.

Issue one blinded permit per eligible natural person/scope. Retries bind the identical blinded request. Redemption verifies the pinned scope key, frozen scope and allowed typed responses, and atomically consumes the scoped random nullifier once.

Other polls, Companies, cohorts or revisions cannot accept it. Eligibility comes from the signing key’s scope, not an invisible client-selected field inside the blinded message.

Separate authenticated issuance from credentialless redemption on an isolated origin without Account cookies, tracking pixels or joinable proxy/access/error/trace logs.

Encrypted ballots contain scope/coarse cohort, random nullifier and sealed typed values—not identity or wall-clock submission time.

Infrastructure necessarily observes traffic transiently. Disclose that the threat model excludes a malicious operator, compromised client/infrastructure, colluding voters and identifying outside knowledge. Do not claim perfect mathematical anonymity from k-anonymity.

Fixed collection windows follow issuance. Batch/shuffle encrypted records; publish only after closure.

Retain the inherited **k=5 minimum**, which cannot be lowered. Freeze one disjoint coarsened partition; do not expose raw-row filters, intersecting axes or exact cross-survey differencing.

At closure, publish one immutable aggregate vector only for cells with at least five responders. Merge/suppress unsafe cells and suppress totals/complements that would recover them.

Every permitted view/export is a union of the same published safe cells, preventing cumulative queries from constructing smaller cells. Do not expose hidden cells through unconditional overall means/totals.

Cross-period comparisons require the same safe basis and are denied when changed membership or complementary releases isolate fewer than five people.

Small/incomplete surveys remain collected but results withheld with an explanation, never invented averages.

Coordinators see privacy-safe participation/reminder counts, not named nonresponders. Send neutral reminders to the frozen electorate; clients holding spent permits may locally suppress their own reminder. Do not join named submission sets to ballots.

Preserve pending blinded requests/permits in a client-encrypted private server envelope under a user-held recovery secret. The service cannot read unblinded contents. Losing that secret does not permit reissuance or double voting; expose the limitation and offer a governed replacement survey.

Retain raw encrypted ballots only through closure verification, default **seven days**, then destroy them and recovery copies. Retain necessary disclosed aggregates under the selected record class.

Raw ballots have no product export/eDiscovery search route. Mandatory lawful process goes through the privacy/legal custodian, not a blanket exemption claim.

Test cross-scope/replayed permits, retries, concurrent redemption, cohort tampering, cumulative/complementary queries, thresholds, telemetry joins and recovery without duplicate voting.

### Official institutions and delegated credentials

Use `sync` at `7b93631e296cba51e23cf05993ab05b267cf7ae3` as endpoint, format, certificate/signing and error evidence only. Its architecture, inventory count and approval convention do not override this product.

Implement NHIS, NPS, HomeTax, four-insurance/si4n and COMWEL employer workflows, plus authorized official banking account/transaction/reconciliation/prepared-remittance flows.

Maintain capability-level inventories covering mandate/authentication; immutable retrieval; schema/version/paging/cursor; amendments/tombstones; evidence; preparation/signing/submission; status/rejection; correction/cancellation; reconciliation.

Test actual formats and supported official sandbox/test paths. Portal-wide green status does not replace per-capability proof. No fabricated certificates, anti-automation bypass or unofficial scraped substitutes.

Read, consequential notice receipt, certificate operation, filing, banking remittance and subscription collection are separate capabilities.

Background unread notices are **metadata-only**.

Opening/actively acknowledging a notice is explicit and attributable, but unread does not necessarily mean legally unserved.

Separately record delivery, institution storage, deemed service, discovery and human opening, with source evidence and institution-specific deadline rules.

National Tax Basic Act Article 12(1), current `lsiSeq=288571`, effective 2026-08-11, may deem electronic service at designated-email entry or tax-network storage before opening.

Metadata acquisition records available service/deadline evidence without opening the body. Missing evidence remains uncertain and escalates; discovery never silently restarts deadlines. “Mark unread” does not undo service.

Test unread-but-served, delayed discovery, disputed service and legal-rule changes.

Existing OpenBao holds institution-specific delegated credentials with purpose-scoped leases, rotation/expiry/revocation and mandate evidence. No secrets in browsers, source control, queues or diagnostics.

Preparation/signing/submission/reconciliation executors are real and tested. Consequential live executors/roles/egress are absent or disabled initially.

Activation is a separately approved operation after mandate, official access, legal requirements and integration evidence. This is an activation gate, not permission for stubs.

### Billing and admission defaults

Immediate free signup provides:

- One Group/Company.
- Three members.
- 250 MiB pooled retained content.
- 1,000 active native records.
- Ten automation definitions.
- One background job.

These are explicit operating defaults, not user-provided numbers. Passkey signup and low-risk work require no human approval.

Representation, high-risk independent approval, mail DNS/abuse checks and institution mandates remain separate gates. Never delete history/statutory records to enforce quotas.

Implement versioned plans/terms/entitlements, seats/storage/usage, prospective changes, invoices/credits/bank instructions and independent transfer reconciliation.

Free price is KRW 0. Do not invent paid prices. Live collection remains disabled. A bank transfer is not automatically an authorized invoice match or entitlement change.

Initial global ceilings:

- 10,000 registered people.
- 400 GiB canonical content.
- 1 TiB total PostgreSQL, subject to lower measured safe admission.
- 2 GiB streamed native files.
- 100 MiB Office sessions.
- 40 MiB MIME.
- Four heavy jobs globally, at most two per Company.
- Verification load: 50 Office coauthors over ten documents.

These are configurable policies and acceptance targets, not measured capacity or license claims.

## 5. Compliance, infrastructure, migration and delivery proof

### Applicable law and records

Implement compliance through versioned applicability, preventive controls, evidence, review and remedies—not a “compliant” badge.

Legal packs record official sources/versions, effective/transitional dates, employer/controller/processor role, headcount/work regime, actual supervision, tax/insurance scheme, reviewer and independent golden results.

A proposed future rate does not become law merely when its date arrives. Missing facts/conflicting rules create resolution work, not zeros or silently omitted workers.

Sources checked on 2026-09-22 include PIPA (`lsiSeq=283839`, decree `289537`), Labor Standards Act (`283457`, decree `270551`), Commercial Act (`273629`, decree `288205`), Electronic Signature Act (`236201`), Electronic Documents Act (`236053`) and dispatch precedents `precSeq=177635` and `235345` at law.go.kr.

Safeguards amendment `admRulSeq=2100000281400` delays Article 8(1)–(2) to 2026-10-31; apply preceding applicable text before then. Recheck official effective instruments for each deployable legal pack.

Required controls:

- Per-field purpose, lawful basis, necessity, recipients and retention, including originals, hidden cells, indexes, archives and derived data. Consent is not a universal RRN basis.
- Access, correction, deletion, restriction, objection and human reconsideration, including applicable PIPA 37-2 deterministic decisions. Request types have distinct deadlines/exceptions.
- Actual subcontract/dispatch/own-factory facts and safety obligations. Preserve contractor discretion and lawful emergency instructions. Contract labels do not determine actual dispatch.
- Site closure does not automatically terminate/reset employment. No universal 52-hour/6-day/11-hour rule for every regime.
- Exact work/break intervals, statutory/contractual leave and accrual-lot promotion/service evidence. Never reactivate the under-30-worker extra-eight-hour exception that expired in 2022.
- Separate ordinary/average/minimum/tax/NPS/NHIS/LTC/employment/workers-compensation bases, caps/rounding, national/local tax and DB/DC/IRP/severance.
- Apply the 2024-12-19 ordinary-wage rulings and transitions, not superseded universal fixedness tests.
- Pin official NTS/NPS/NHIS/COMWEL schedules and scheme-specific deadlines. Health-insurance loss is not universally due next-month-15.
- Separate payslip issue/delivery, acknowledgment and settlement. Preserve exact artifacts and signing intent. Passkey authentication/UV alone does not prove biometric authentication or legal assent.
- Corporate resolutions, charter-dependent quorum/conflicts/minutes, accounting evidence and representation use actual authority, not titles.
- Controller/processor contracts, subprocessors, overseas access, scoped support and incident response. Start awareness clocks and implement applicable reporting/notice deadlines, including relevant 72-hour rules and exceptions.
- Korean hosting alone does not prove absence of overseas transfers.
- MyData requirements follow actual designated/regulated roles; no blanket SaaS license/exemption claim.
- Institution mandates/terms govern credential use and activation.

### Retention, holds and destruction

Retention is record-class and event-based. Overlapping obligations retain only necessary records under their schedules—not whole profiles for the longest related deadline.

| Record class | Required/default clock |
|---|---|
| Employee roster / employment contract | Minimum three years from applicable separation/death / relationship end |
| Wage ledger / calculation-support papers | Minimum three years from last entry / completion; separate tax/commercial duties remain |
| Other employment/leave/retirement records | Enumerated statutory/plan classes and their own event clocks |
| Important corporate books / business vouchers | Ten years / five years with correct statutory triggers; specific minutes/continuing-record requirements remain |
| National-tax books/evidence | Baseline five years from relevant statutory filing deadline under Article 85-3; longer exceptions/local tax separately sourced |
| Personal-data access / permission history | Applicable sensitive/unique-ID HR access logs at least two years; grant/change/revoke history at least three years; applicable monthly review |
| Applicant materials | Applicable return claim window within 14–180 days; operating default 30 days, then actual return/destruction. Electronic/nonreturnable submissions and optional pools have separate schedules |
| Ordinary communications | Default one year per message/version from durable acceptance. Unrelated room/thread/mailbox activity never resets it. Retain a family while a retained version, legal record or hold requires it |
| Plaintext export staging / diagnostics | Staging removed on confirmed delivery, maximum 24 hours; redacted diagnostics 30 days, separate from statutory audit |
| Backups / holds | Up to 35 days only where lawful for the class; no blanket deletion exemption. Scoped holds reviewed every 90 days and explicitly released |

Attachments expire only when no surviving authorized retained reference or hold requires them. References do not broaden access.

Destruction receipts distinguish controlled-system copies from external-recipient copies requiring separate lawful requests/attestation. Do not promise remote destruction of received plaintext.

Propagate deletion to replicas, caches, indexes, exports, archives and backup disposition. Restore remains isolated until independently preserved erasure/revocation ledgers are reapplied and verified.

Do not claim crypto-erasure while recoverable key backups exist.

Where a shorter mandatory deadline conflicts with backup retention, expire/rebuild affected sets and recoverable key history within that deadline while preserving clean recovery copies. Do not report “deleted” while required destruction is pending.

Holds racing deletion have real draining/partially-removed states. A successful HEAD check is not proof of complete custody.

### Deployment topology

Use the three continuously available South Korean sites. Apple and AMD are confirmed to have independent buildings, power and Internet circuits, public ports/fiber and UPS; verify this operationally before commissioning.

| Site | Role and guardrail |
|---|---|
| **7800X3D, 32 GB, 3 TB SSD** | Native x86_64 PostgreSQL primary; bounded edge/core/mail/archive device and backup repository. At most 28 GiB for v1 workloads. Keep heavy conversions/analytics off during transactional peaks |
| **Apple Silicon 18-core, 128 GB, 6 TB SSD** | x86_64 Linux QEMU physical PostgreSQL standby with compatible build/extensions/collation; native ARM Linux services for Next/Rust/Synapse/Euro/data/conversion, second archive device and backups. Start DB guest at 48 GiB with bounded pools/headroom |
| **Existing OCI A1, 4 OCPU/24 GB/100 GB, Chuncheon-1** | Preserve Talos/OpenBao. Bounded namespaced etcd witness/coordination only; no full mail/object database. Initial added cap 0.5 OCPU/1.5 GiB/8 GiB, subject to actual allocation |

Never destroy, terminate, resize or reprovision grandfathered A1 capacity. Do not implicitly join/reconfigure the existing cluster.

New workloads use isolated Linux guests, systemd/rootless Podman and pinned OCI images. Allocate guests without erasing unrelated host work. No stretched Kubernetes/L2 assumptions or counting VMs as independent sites.

If a host currently uses Talos, use its supported VM facility or an explicitly authorized host change. Never silently reinstall.

Use plain PostgreSQL, three etcd members—one per site—and **manual promotion**, not Patroni/custom automatic failover.

The lease binds system ID, primary, timeline and monotonic epoch. Restart/rejoin is quarantined, never auto-primary.

Promotion requires positive fencing of old writer and egress, plus safe-WAL/operation reconciliation. Missing fencing means no writable promotion.

One surviving data site provides read/recovery only until a second independent full replica is healthy and confirmation barriers pass. No invented failover-write SLA.

### Safe reads during failure

For normal reads, materialize the authorized bounded response/snapshot, capture a post-read WAL INSERT-LSN bound and pass the same two-site barrier before publishing. Revalidate emission authority/epoch.

This prevents unconfirmed local commits escaping through GET, search, IMAP, sync or realtime.

A dedicated read-only durability observer records monotonic safe-prefix certificates in authenticated durable etcd storage:

- PostgreSQL system identifier.
- Timeline and ancestry.
- Writer epoch and safe LSN.
- Authenticated primary-flush and independent standby-durable-replay observations.

A certificate describes WAL already durable at both full sites. Its own storage does not recursively require inclusion in the business WAL prefix. Lost newer certificates reduce readable freshness, not preservation of acknowledged records.

Business runtime roles cannot mint certificates or move them backward.

During a one-site outage, business reads come only from a **separate read-only recovery clone**, restored from a compatible base at/before the certified LSN and paused at that recovery point.

Never filter arbitrary current-primary rows by LSN or expose its unconfirmed tail. Preserve later WAL and operation evidence for reconciliation; do not truncate it to construct the clone.

Display as-of/freshness. Current identity, revocation and object/field authority must be independently provable; uncertainty fails closed.

Without a valid certificate/coherent clone, business reads remain unavailable until safe reconstruction.

Unsent browser drafts may remain in memory, but no durable-save claim is made without two-site confirmation.

### Backups, resource admission and operations

Use encrypted pgBackRest cross-site backups and continuous WAL, with separate restore credentials/repository/operator authority.

Keep backups on both physical sites within admission limits. Test coherent restore of native/OSS databases, immutable chunks, Office changes, mail queues/config/keys and both Matrix stores.

OCI Object Storage is optional extra capacity only after actual free-quota/cost/retention verification. Do not assume the full dataset fits free. Replication is not backup.

Admission accounts for DB/index/history/WAL/backup/temp footprint, not only customer bytes. Reserve at least **20% free space** on each full data site.

Alert before throttling; never delete legal records to recover capacity. One-TiB DB/400-GiB content are upper ceilings, automatically reduced where backup/WAL/headroom cannot fit. Stop bulk ingestion before endangering transactional records.

Require TLS/WireGuard, default-deny network/egress, scoped OpenBao, rotation, signed images/SBOM/provenance, patching and restore rehearsals.

Reuse OpenBao without extracting live secrets during development.

Collect redacted OpenTelemetry/Prometheus metrics and logs, correlating operation IDs to separately authorized evidence. Monitor replication/epochs, UNKNOWN outcomes, queues/fairness, archive admission/keys, conversion, source freshness, storage, mail abuse/DNS, certificates, backup age and actual restore proof.

### Verification targets—not current performance claims

- 10,000 registered identities.
- 500 active browsers and 100 aggregate API requests/second: 80 read/20 ordinary mutation.
- 50 Office coauthors over ten documents.
- 25 message sends/second plus bounded mail/import/background work.
- At that mix: p95 authorized read ≤500 ms; ordinary durable mutation ≤1 second; archived send ≤2 seconds; normal Office durable changes ≤1 second.
- Expensive calculations/exports are asynchronous with truthful progress.
- 10,000-person ordinary monthly payroll ≤15 minutes with full population accounting; test complex/retro cases separately.
- Zero confirmed-record loss under loss of either full data site.
- Recovery/read target ≤60 minutes when safe fencing is available.
- Writes resume only after two full copies recover.
- Internal monthly API objective 99.5%; customer SLA is separately agreed.
- Before launch: 72-hour mixed-workload soak, maximum admitted-size restore and seven-day operational rehearsal.

Targets never justify weaker durability, omitted modules or hidden partial work.

### Independent fork and no-write-pause v2 cutover

Freeze an allowlisted closure of Console’s actual working tree: exact HEAD, staged/unstaged/untracked relevant hashes, Cargo dependencies, migrations, contracts, tests and tooling.

Observed reference HEAD: `05200e43af13b527ce031b04369e05852862f1d6`. It is not a substitute for dirty-tree closure.

Rehash before/after copying and retry on drift. Exclude secrets, live data, caches and unrelated work. No HEAD-only archive shortcut, live symlink or automatic upstream write.

Keep frontend as the independent Next v1 product with copied backend closure and minimal fork-delta ledger. Preserve applied migration bytes/checksums.

Keep fixtures outside production artifacts. Test empty installation and populated upgrade. Do not carry seedful installation into production or “fix” it by changing applied migrations.

Full-state migration includes:

- Accounts/People/Companies, passkeys/RP, terms/consents/sessions/revocations.
- Object/source/dataset versions and lineage.
- Drafts, approvals, tasks/timers/jobs.
- Idempotency keys, outboxes, attempts and UNKNOWN outcomes.
- Evidence/blobs/tombstones/holds/erasure.
- Mail UIDs/queues/delegation/DKIM.
- Matrix IDs, memberships, events, devices/cross-signing/backup state.
- Office document IDs/change logs.
- Calendars/recurrences.
- Billing/entitlements.
- Package/customization/version history.

Use **expand → backfill/CDC → reconcile → shadow comparison → per-Company owner-epoch handover → compatible rollback window → separately authorized contraction**.

One live business writer and one egress authority per ownership partition. Never dual-execute payments, filing, messages or notifications.

Ingress durably accepts authenticated commands during planned cutover. Old work drains under its owner; queued commands retain IDs and move to the new epoch with current-policy checks.

Expose genuine queued/pending status without a scheduled maintenance/write-rejection window.

OSS stores may stay unchanged across UI/business-engine cutover. Later OSS replacement is a separate full-state migration.

Reconcile exact counts, typed hashes, per-person liability/payment coverage, ledger sums, inventory quantities, evidence versions and queued/unknown effects. `n_live_tup` is not exact proof.

Exercise ongoing traffic, long drafts, mixed clients, webhook races, period close, key rotation, membership revocation and rollback.

Rollback forwards/replays accepted post-cutover changes into the compatible old path before restoring ownership. No PITR rewind discarding accepted writes, disabled RLS, identity reset or resurrected credentials.

### Implementation order

The grounding record distinguishes reviewed requirement documentation from executable prototypes, generated logs/arrays and runtime/vendor artifacts.

The inherited review covered the original 799 documentation paths/768 byte-unique corpus, six frontend documents and 23 sync Markdown documents, plus supplemental documentary JSON. Remaining native-interview metadata wrappers were completed: 117 files, 135,825 bytes.

This is not a claim that every raw HTML prototype, generated artifact or source file was read line-by-line. Pin relied-on requirement sources and resolve historical conflicts against explicit v1 decisions and current official law. Historical approvals or green prototypes are not readiness evidence.

Use lean-build/ponytail reuse and simplicity, plus migration compatibility/rollback discipline. These eliminate duplicated engines—not required product depth.

1. **Persist approved planning artifacts and freeze the source closure.** Establish reproducible builds, exact test discovery and red missing-behavior probes. Remove production seed/stub/fallback reachability without deleting capabilities or weakening tests.
2. **Foundation.** Complete identity/Company isolation, action/draft/review/receipt, current authority, storage/durability/fencing, legal-rule versioning and real SSR shell. Prove clean install/populated upgrade and direct-protocol negatives.
3. **Business workflows.** Complete B01–B23 and leaf workflows. Start organization → contract/site → staffing/time → payroll/billing/GL to establish shared invariants. Deliver records, corrections and handover alongside consuming workflows.
4. **Platform and collaboration.** Complete P01–P15, connectors, data/ontology/rules/apps/code/packages, Euro, Stalwart and Matrix. Prove existing-tenant upgrades, same-action imports, source-security propagation and cross-module journeys.
5. **Integrated release proof.** Independent security/legal-domain review, actual hardware capacity, restore/cutover rehearsals and full launch checklist. Required modules launch together. Exposure, live institutional acts and collection remain separately authorized.

### Executable acceptance

For each B/P row and leaf, bind:

**source requirement → responsible owner → screens/actions/contracts → positive/negative/concurrent/failure/recovery tests → command/build/input evidence**

No generic “module complete” checkbox substitutes for terminal outcomes.

Every visible action, input, dialog, export and deep link needs real-API browser proof. Sort/filter/count must cover the dataset, not silently the current page.

Required suites:

- Zero business facts on clean production boot; no reachable demo factories, dev-auth, stub providers, `fallbackData` or fake health. Fixtures remain isolated.
- Real passkey ceremonies, identity continuity/recovery/terms, natural-person independence, revocation, issuer/subject SSO and SCIM lifecycle.
- Positive authorized journeys and cross-Company/object/field/list/detail/search/export/realtime denial.
- Payroll/leave/statutory goldens, close-vs-mutation concurrency, balanced ledgers, duplicate invoices/payments, inventory reservation/split/merge, transfers, appeals and full batch coverage.
- ZIP bombs/oversize, formula/CSV/HTML injection, SSRF/rebinding, malformed Office/HWP/PDF, hidden sensitive cells, code escape/egress, schema/API drift and audit forgery.
- Stalwart all-protocol isolation/revocation, no relay/silent drop, MIME/queue consistency and lost remote-250 status.
- Euro concurrent saves/replacement/chunk failures/revocation/restarts with exact recovery.
- Matrix crypto-generation/wrapping-key recovery, browser encryption/exclusive ownership/logout/refresh/recovery, acknowledged orphan uploads, missing keys, replay/re-encryption, attachments, edits/redaction and all final-guard paths.
- Rights/retention/hold races, message/attachment expiry, legal export/external copies, backup-erasure resurrection, poll/survey privacy/recovery, employee-relations protections and incident/service clocks.
- Connector success/empty/denied/failure/drift/partial/pagination/replay/late update/tombstone, unread-notice metadata, mandate/certificate revocation and partial/UNKNOWN correction.
- Real responsive, keyboard, screen-reader and Korean IME journeys with separate authenticated users. Mock APIs and synthetic passkey fixtures alone are insufficient.
- Cancel synchronous waits; isolate either site; partition etcd; kill stale senders; crash during claim/ACK/archive/save; fill disks; restart old snapshots; restore maximum admitted size; execute no-pause cutover/rollback under traffic.
- Independently verify zero acknowledged-record loss and no unsupported exactly-once/unauthorized-execution claims.
- Record expected, discovered, executed, skipped and unreached tests accurately. Required failures are fixed, not quarantined.

Baseline commands remain `npm ci`, `npm run lint`, `npm test`, `npm run build`, `npm run test:e2e` and locked Cargo workspace/unit/PostgreSQL integration checks in the isolated fork.

Add one acceptance runner for the B/P matrix, real-service browser/protocol, fault, capacity, restore and migration scenarios. Emit exact commands, discovery/execution counts, hashes and outcomes.

Tests must not fabricate provider success or perform unauthorized live institutional mutations.

### Approval, persistence and stop conditions

Independent reviewers examined the complete plan, raised concrete counterexamples and reviewed revisions without suppressing objections or weakening requirements.

Final R4 consensus:

| Reviewer | Verdict |
|---|---|
| Product/workflows — `final_product_review` | ACCEPT |
| Compliance/security — `final_compliance_review` | ACCEPT |
| Architecture/durability/migration — `final_architecture_review` | ACCEPT |

All three verified the same R4 SHA-256. Historical/scoped approvals were not counted.

Before implementation, save:

- `docs/planning/nextjs-v1/PLAN.md` — exact approved specification.
- `docs/planning/nextjs-v1/requirements-and-evidence.json` — source/requirement/owner/screen/contract/test/evidence index.
- `docs/planning/nextjs-v1/reviews.json` — versions, hashes, findings, dispositions and independent verdicts.

No repository files have been written during this planning turn. When file writing is permitted, persist these artifacts before copying source or changing product code.

Deployment inventories, free quotas, legal-pack data and load/restore outcomes remain execution inputs and acceptance evidence—not facts proven by planning.

Implementation stops for material design changes, violated invariants, unsafe migration/rollback, missing authority or failed required acceptance. Such findings return to the relevant review; they are not hidden by disabling required functionality.
