# People & Workforce Administration

`POST /api/v1/employees` is the governed HR-admin creation path. It records a
tenant-scoped employee identity and employment profile, an `ONBOARD` lifecycle
event, and the audit event in one database transaction. `idempotency_key`
replays the original payload; a reused key with different normalized content is
a conflict. A tenant-scoped reservation row is inserted and locked before any
employee write, so concurrent same-key requests create one employee, lifecycle
event, and audit event; the waiter receives the persisted replay.

The ordinary `GET /api/v1/employees` directory is paginated and intentionally
does not return phone or compensation. Those values are available only from
the HR-manager-only `GET /api/v1/employees/{id}` detail endpoint. Both paths
authorize before querying and query through tenant RLS, so a caller receives
neither cross-tenant data nor a count/object existence signal.

## Delivered workflow

`web/src/console/people/PeopleWorkforceBody.tsx` calls only those real
endpoints. It loads an authorized directory and active branch identities,
offers bounded suggestions derived from the authorized directory while keeping
the contract's direct-entry fields available, and creates the employee with a
stable idempotency key retained across a failed retry. Korean local phone input
is normalized to E.164 and KRW input to the canonical decimal before submit;
the server remains the authoritative validator.

A successful create and a deliberate directory-record open both render the
persisted privileged detail returned by the backend, including normalized phone
and base pay. The directory itself never receives or synthesizes those fields.

## Completion boundary

This module owns the create/list/detail workflow, migration `0172`, OpenAPI,
and generated clients. Mounting this body into the console screen registry and
its route/nav authorization is owned by the integration lane; this module does
not claim that route seam as complete until it is mounted and end-to-end tested
there.

`PEOPLE_WORKFORCE_ROUTE` exports the exact `people` screen key,
`/console/people` path, and component for that root-owned mount without
duplicating shell routing.
