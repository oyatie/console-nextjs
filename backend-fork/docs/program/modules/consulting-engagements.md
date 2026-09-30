# Consulting engagement and realized benefit pilot

`/api/v1/consulting/engagements` is a rollout-dark, tenant-scoped pilot for a
closed loop: a sourced customer engagement and diagnostic, evidence-backed
findings, a KPI-definition-referenced initiative, an authoritative approval,
implementation review, an evidence-linked observation, and a sustained or
corrective decision. The console is deliberately read-only until its authorized
customer/document/KPI/evidence selectors and approval-request handoff exist.

The module retains sourced references rather than KPI values; reporting remains
the KPI authority. Customer references are constrained to the tenant. Approval
transitions atomically consume a matching, unused four-eyes approval bound to
the engagement. Every mutation writes an immutable
`consulting_engagement_history` row, transitions use the returned `version` as
an optimistic-concurrency token, and creation is idempotent per tenant key. All
six tables have forced PostgreSQL RLS keyed to `app.current_org`; because
engagements have no branch/site scope, access requires organization-wide
authority.

Read access uses the existing management-read policy gate and writes use the
existing management-write gate until the policy catalog gains dedicated
`consulting_read` and `consulting_manage` enum entries. The database feature
catalog rows are already seeded, so that policy extension is a narrow follow-up,
not a client-side authorization bypass.
