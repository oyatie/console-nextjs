> **SUPERSEDED PRE-PIVOT SPEC:** This is a frozen record for the deleted React,
> generated-client, field-approval, and E2E surfaces. It is not current product
> scope, a runnable command guide, or implementation authority. Do not modernize
> individual commands and mistake the result for an active spec. Current authority
> is [`docs/PIVOT-2026-07-28.md`](../PIVOT-2026-07-28.md).

# Spec: Issue #55 federated Approval Command Center slice

## Objective
Build the next issue #55 enterprise-collaboration slice by making `/approvals` a server-owned, policy-scoped Approval Command Center. The page must consume one federated approval API, preserve existing source-specific decision endpoints, display real pending target-change requests, and carry enough ontology/workflow/policy context that future PBAC/ABAC/RBAC, passkey step-up, audit, and optimization layers can reason over the same objects.

## Tech Stack
- Rust/Axum work-order REST service under `backend/crates/workorder/rest`.
- OpenAPI contract in `backend/openapi/openapi.yaml` with generated TS/Kotlin/Swift clients.
- React + TypeScript web console under `web/src` using `createConsoleApiClient` and `useAuth().api`.
- Existing production decision endpoints remain source-specific:
  - `POST /api/work-orders/{workOrderId}/approve`.
  - `POST /api/v1/work-orders/{workOrderId}/reject`.
  - `POST /api/target-change-requests/{requestId}/review`.

## Public API contract
`GET /api/approval-items?limit=&offset=` returns `ApprovalItemsPage`:

- `items[]`: stable federated approvals from work-order report review, daily-plan review, and target-change review.
- `sources[]`: server-computed counts for `workOrders`, `dailyPlans`, and `targetChanges`.
- `total`, `limit`, `offset`: pagination envelope.

Each `ApprovalItem` carries:

- stable federated id: `{SOURCE}:{source_id}`;
- `ontology`: object type/id, tenant/org id, branch id;
- `workflow`: workflow key and action key;
- `policy`: server-side allow decision, enforcement marker, required capability/features, and branch scope;
- exactly one source payload: `work_order`, `daily_plan`, or `target_change`.

The backend enforces approval-list visibility before returning data. Source-specific mutations must still re-authorize at their own endpoints before changing state.

## Commands
- API client drift: `npm run gen:api:portable && npm run gen:api:swift`.
- Targeted unit/component tests: `npm --prefix web test -- ApprovalsPage.test.tsx ApprovalQueue.test.tsx WorkHubPage.test.tsx`.
- Broader web route regression: `npm --prefix web test -- App.test.tsx`.
- Lint: `npm --prefix web run lint`.
- OpenAPI app check: `npm run check:openapi-app`.
- E2E smoke: `PATH="$PWD/.venv-e2e/bin:$PATH" bash e2e/run.sh e2e/specs/admin-07-approvals.spec.ts e2e/specs/admin-08-daily-plan.spec.ts e2e/specs/admin-19-stub-wiring.spec.ts e2e/specs/admin-21-work-hub.spec.ts`.

## Project Structure
- `backend/crates/workorder/rest/src/lib.rs` owns the federated approval-list route, authorization, SQL union, and response mapping.
- `backend/app/tests/workorder_api.rs` proves branch-scoped federation and mechanic denial.
- `backend/openapi/openapi.yaml` is the source of generated client contracts.
- `web/src/pages/ApprovalsPage.tsx` orchestrates federated reads and source-specific writes.
- `web/src/features/approvals/ApprovalCommandCenter.tsx` renders aggregate approval-source cards and pending daily-plan links.
- `web/src/features/approvals/ApprovalQueue.tsx` remains the source-specific work-order decision queue.
- `web/src/features/approvals/TargetChangeReviewQueue.tsx` renders real pending target-change requests from the federated API.
- `web/src/pages/ApprovalsPage.test.tsx` covers the command-center behavior, legacy endpoint avoidance, retryable failure, and deep-link focus.
- `docs/benchmarks/issue-55-collaboration-work-hub.md` tracks shipped slices and remaining valid gaps.
- `docs/specs/korean-legal-boundaries.md` captures Korean privacy/labor/location/legal guardrails for future HR/payroll/location/signing work.

## Code Style
Use typed props, generated API types, real source labels, and existing design-system primitives. Keep data fetching in the page, presentation in feature components, and authorization/policy decisions on the server.

```tsx
<ApprovalCommandCenter
  workOrders={workOrders}
  dailyPlans={dailyPlans}
  targetChanges={targetChanges}
  sources={approvalPage?.sources ?? []}
/>
```

## Testing Strategy
- RED first: backend test proves `/api/approval-items` rejects mechanics, returns only branch-visible approvals, includes all three source types, and provides source counts.
- RED first: page test proves `/approvals` calls only `/api/approval-items`, renders counts, deep-links requested daily plans, lists target-change requests, and preserves stale/focused work-order deep-link behavior.
- Existing `ApprovalQueue` tests continue proving evidence-before-decision and memo-scoped rejection.
- Generated-client drift checks must pass before merge.
- E2E must continue proving real approve/reject and target-change review flows after deployment.

## Legal / compliance boundaries captured for this slice
This is an engineering guardrail, not legal advice or counsel sign-off. It keeps issue #55 aligned with the broader Korean compliance posture:

- Personal data and employee data must stay purpose-tagged, minimized, permissioned, retained/deleted by policy, and disclosed through the privacy policy/consent surfaces. Sources: Korean Personal Information Protection Act and PIPC 2026 privacy-policy guidance.
- GPS/location approvals and work telemetry must remain consent/purpose/retention gated; a work-order approval item may link to location evidence only through authorized source-object context, not by leaking raw continuous tracking.
- HR/payroll/retirement fields are high-sensitivity domains. Approval cards may reference that a payroll/HR object exists, but sensitive amounts, bank accounts, resident registration numbers, disability/protected status, or retirement-interim-settlement details require separate domain permission, masking, audit, and passkey step-up.
- Work-hour/overtime/night/holiday/payroll decisions must preserve legal calculation inputs and decision lineage; no UI shortcut may weaken wage-statement, wage-ledger, worker-roster, or labor-record obligations.
- Sensitive approvals/signing-equivalent actions must require passkey step-up before the UI may label the audit trail as signature-grade.

Official source anchors:

- Korea Personal Information Protection Act: <https://law.go.kr/lsInfoP.do?lsId=011357>
- PIPC 2026 privacy-policy guide: <https://www.privacy.go.kr/front/bbs/bbsView.do?bbsNo=BBSMSTR_000000000049&bbscttNo=20885>
- Labor Standards Act: <https://www.law.go.kr/LSW/lsInfoP.do?ancYnChk=0&lsId=001872>
- MOEL wage-statement calculator/guidance: <https://www.moel.go.kr/wageCal.do>
- Location Information Act: <https://www.law.go.kr/LSW/lsInfoP.do?lsiSeq=277359>
- Employee Retirement Benefit Security Act: <https://www.law.go.kr/LSW//lsSideInfoP.do?docCls=jo&joBrNo=00&joNo=0009&lsiSeq=279829&urlMode=lsScJoRltInfoR>

## Idea refinement: operations optimization is valid, but not this PR

### How might we...
How might we turn trusted operational objects into decision-grade recommendations for assets, rentals, reserves, workforce, pricing, and lifecycle choices without contaminating the current setup/approval slice with speculative analytics?

### Recommended direction
Build optimization as a governed analytics layer over canonical objects and audited workflows. The foundation is not a dashboard; it is reliable ontology/policy/action lineage: assets, rentals, customers, sites, parts, workforce, costs, SLAs, approvals, and source events must be modeled consistently before recommendations can safely write back.

This PR contributes a small but necessary seam: approval items now expose object identity, workflow keys, policy requirements, tenant/org scope, and source-object links. Future optimization engines can then attach recommendations to objects and route proposed actions through the same approval/signing/audit path.

### Key assumptions to validate
- Managers will trust recommendations only if every recommendation shows source data, assumptions, confidence, and expected business impact.
- Asset/rental optimization requires financial cost ledgers, market value snapshots, utilization, downtime, SLA penalties, and reserve demand data; missing any of these should produce “insufficient evidence,” not fake precision.
- Write-back must stay governed: a recommendation can draft a price change, reserve policy, acquisition/sale request, or workforce plan, but a human/policy workflow must approve it.

### MVP scope for future backlog
- Decision objects: `Recommendation`, `Scenario`, `AssumptionSet`, `OptimizationRun`, `DecisionApproval`.
- First domains after setup: rental pricing, asset sell/keep/acquire, reserve equipment/parts sizing, workforce utilization.
- UI pattern: executive/manager scenario workspace with impact, constraints, confidence, source lineage, and approval action.

### Not doing in issue #55
- No pricing optimizer, forecasting model, market-value estimator, or reserve-policy solver in this PR.
- No fake analytics cards without source data and validation.
- No optimization write-back that bypasses policy, passkey step-up, or audit.

## Boundaries
- Always: use generated API types, render meaningful loading/error/empty states, use Korean i18n strings, preserve existing approve/reject handlers, and keep authorization/audit decisions server-owned.
- Ask first: destructive production data changes, new external vendors, new auth/passkey protocol, schema changes outside the current approval contract.
- Never: mock approval sources in production UI, claim passkey step-up is enforced by this slice when it is not, bypass backend authorization/audit checks, or expose HR/payroll/location-sensitive data through generic approval cards.

## Success Criteria
- `/approvals` shows an enterprise command-center summary across work-order reports, daily-plan review, and target-change review from `GET /api/approval-items`.
- Requested daily plans are visible on `/approvals` with deep links to `/daily-plan?planId=...`; non-requested plans are not counted as pending approvals.
- Target-change requests are listed from real backend data and can be approved/rejected through the existing review endpoint.
- Work-order approve/reject flow continues to use existing backend endpoints.
- Mechanic users cannot load the manager/admin approval inbox.
- Branch-scoped users do not see approval items from other branches.
- Generated TS/Kotlin/Swift clients include the new approval contract.
- Targeted tests and lint/e2e smoke are run or explicit gaps are reported.

## Open Questions / Backlog
- Passkey step-up for sensitive approval decisions is product-required but outside this slice; backend/session step-up support must be added before the UI can truthfully mark it enforced.
- Approval cards need richer requester, evidence, conversation/mail links, SLA, and audit-trail metadata as source payloads mature.
- Extend federation to purchase requests, HR/payroll approvals, policy changes, mail-derived work, support escalations, and future optimization recommendations.
- Add policy-admin UI for PBAC/RBAC/ABAC rules so managers with permission can configure roles, attributes, and action policies without code changes.
- Add legal-review checkpoints before enabling payroll calculation, wage statements, retirement/severance/intermediate-settlement workflows, resident-registration handling, or continuous location tracking.
