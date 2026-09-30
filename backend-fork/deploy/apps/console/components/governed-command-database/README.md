# Governed command database component (DARK)

This opt-in Kustomize component stages the PR 473 seven-role PostgreSQL topology
without changing the live `base`, `prod` overlay, or Argo CD `Application`. No
live Application references this directory. A merge, render, or test is not
deployment authorization and does not prove migration, readiness, rollback, or
production operation.

## Portable port and substrate adapters

The application-facing port is stable across substrates:

- `DATABASE_URL` authenticates as `console_rt`;
- `LEAVE_COMMAND_DATABASE_URL` authenticates as `console_leave_cmd`; and
- `ONTOLOGY_COMMAND_DATABASE_URL` authenticates as `console_ontology_cmd`; and
- `PLATFORM_FORCE_COMMAND_DATABASE_URL` authenticates as `console_platform_force_cmd`.

Each Kubernetes Secret exposes the same `username`, `password`, and `uri` keys.
The secret-store and infrastructure adapters vary by deployment context:

| Context | Secret adapter | Database and object-store adapter |
|---|---|---|
| self-host/on-prem | OpenBao HA Raft through External Secrets Operator | on-prem CNPG, storage, network policy, and self-hosted S3 choices |
| OCI guest | OCI Vault values projected manually into typed Kubernetes Secrets | current OCI CNPG and OCI Object Storage choices |

Self-host is the first activation target. OCI remains first-class and uses its
native recovery and object-storage capabilities without leaking OCI-specific
requirements into the workload Secret contract.

## Seven-role contract

CloudNativePG's inaccessible `postgres` administrator is not an application
role. The component reconciles these seven application roles:

| Role | Login | RLS | Purpose and privileges |
|---|---:|---|---|
| `console_app` | yes | `BYPASSRLS` | migration-only database owner; runs tenant-wide backfills and DDL; member of both definer roles without admin option |
| `console_rt` | yes | enforced | general API and worker DML; owns no tenant tables |
| `console_leave_cmd` | yes | enforced | API-only `EXECUTE` on intrinsically audited leave command routines; no direct table DML |
| `console_ontology_cmd` | yes | enforced | API-only `EXECUTE` on intrinsically audited ontology command routines; no direct table DML |
| `console_platform_force_cmd` | yes | enforced | API-only `EXECUTE` on archived-tenant force removal; no direct table DML |
| `console_leave_definer` | no | enforced | owns the leave command functions; cannot authenticate |
| `console_ontology_writer` | no | enforced | owns the ontology command functions; cannot authenticate |

Every serving login is `NOSUPERUSER`, `NOBYPASSRLS`, `NOINHERIT`,
`NOCREATEDB`, `NOCREATEROLE`, and `NOREPLICATION`. The topology gate rejects
unexpected role membership on a serving identity and proves each credential
opens a direct `session_user = current_user` connection. An admin connection
followed by `SET ROLE` is not acceptable evidence.

Every serving role has exact global defaults of `statement_timeout=30s`,
`idle_in_transaction_session_timeout=30s`, and `transaction_timeout=45s`.
CloudNativePG does not model role-default GUCs, so the wave-1 Job connects
directly as each serving role and self-applies these USERSET values without a
superuser secret. It removes only the three managed keys from every
database-specific override, preserves unrelated settings, verifies exact
catalog state inside the transaction, commits, drains older sessions only for a
role whose managed defaults or overrides actually required repair, and then
opens a fresh direct connection to prove effective values. An exact-state Sync
is readback-only and preserves healthy in-flight serving sessions. PostgreSQL 17 or
newer and `max_prepared_transactions=0` are mandatory; prepared transactions
are exempt from `transaction_timeout`. These controls bound normal operation
but are not a security boundary because each login may change its own USERSET
defaults. Migration-owner, offline, and operator writers remain outside this
control; gap-free audit sealing still requires quiescence/coordination or a
future xmin/snapshot watermark.

## Secret contract

The component consumes these Secrets in namespace `maintenance`:

| Secret | Required keys | Consumer |
|---|---|---|
| `console-db-app` | `username`, `password`, `uri` | CloudNativePG-generated; migration and topology Jobs only |
| `console-db-rt` | `username`, `password`, `uri` | CloudNativePG, API, and worker |
| `console-db-leave-command` | `username`, `password`, `uri` | CloudNativePG and API only |
| `console-db-ontology-command` | `username`, `password`, `uri` | CloudNativePG and API only |
| `console-db-platform-force-command` | `username`, `password`, `uri` | CloudNativePG and API only |

All five passwords must be non-empty and pairwise distinct. Generate URI-safe
passwords, such as 32-byte hexadecimal values. If another alphabet is used,
percent-encode the password component in each PostgreSQL URI correctly. Each
operator-managed database Secret must use type `kubernetes.io/basic-auth` and
label `cnpg.io/reload=true`.

The provider-neutral OpenBao/ESO projections are staged in
[`../../../secrets-management/components/governed-command-database/`](../../../secrets-management/components/governed-command-database/README.md).
The OCI manual projection procedure is documented with the
[`pr-473-expand-oci-guest` overlay](../../overlays/pr-473-expand-oci-guest/README.md).

## Ordered activation contract

The component converts the live PreSync migration into an ordered Sync sequence:

| Wave | Resource | Required result |
|---:|---|---|
| 0 | `console-db` CloudNativePG Cluster | all seven roles and password references reconcile |
| 1 | `console-db-topology` Sync hook | role attributes, ownership, memberships, pairwise credential separation, exact timeout defaults, repair-scoped old-session drain, and fresh direct login identities read back exactly |
| 2 | `console-migrate` Sync hook | embedded migrations finish successfully with the migration-only owner |
| 3 | `console-app` Rollout and `console-worker` Deployment | serving workloads start only after both database gates pass |

Never selectively sync the topology Job, migration Job, API, or worker. Sync the
whole console Application so all prerequisite waves run. A topology or
migration failure must stop the serving rollout.

### Legacy Apalis ownership is an activation blocker

The owner/runtime split is mergeable while this component remains DARK, but an
existing database may contain Apalis objects created earlier by `console_rt`. The
new migration path deliberately refuses any Apalis schema, relation, migration
ledger, function, or vendor `public.generate_ulid()` helper that is not owned by
`console_app`; it also refuses an unledgered partial schema. It never silently
adopts, rewrites, or broad-transfers those objects.

Before any activation proposal against an existing database, a cluster
administrator must capture an exact inventory of the Apalis ledger and every
vendor-created object, rehearse the transition on a restored disposable copy,
and produce an enumerated ownership-transfer manifest plus rollback evidence.
Transfer only the named Apalis objects and vendor helper after the inventory is
reviewed. Do not use `REASSIGN OWNED BY console_rt`, because `console_rt` may own
unrelated objects and a broad transfer would cross the adapter boundary. After
the transfer, the exact candidate's migrate mode must converge the ledger and
ACL, and fresh direct `console_rt` API and worker sessions must prove enqueue,
claim, acknowledgement, retention, and denied DDL. Until that rehearsal and
evidence exist for the selected database state, activation is blocked.

The component stamps pod-template annotation version `0167` on both API and
worker. Whole-Application activation therefore replaces every pooled serving
process after the old-session drain; a metadata-only Deployment annotation is
not accepted as a restart barrier.

## Connection budget

PostgreSQL is configured for 60 connections. The code caps every general runtime
pool at 6 connections and each API command pool at 2. During a worst-case
blue/green API rollout and worker rolling surge:

- four API pods use `4 x (6 + 2 + 2 + 2) = 48` connections;
- two worker pods use `2 x 6 = 12` connections; and
- total serving demand is 60, leaving no headroom for migration, topology,
  CNPG, and operator access.

Treat 6/2/2/2 and the rollout replica/surge settings as one reviewed budget. Do
not raise a pool, replica count, or surge independently.

## Promotion procedure and evidence gates

Activation requires a new, explicitly reviewed promotion change. Complete these
steps in a non-production environment before any production proposal:

1. Select one unreferenced rehearsal overlay and the matching secret adapter.
2. Create a recovery bundle outside the cluster before reconciling credentials.
3. Render the complete overlay and verify that the live `base`, live `prod`, and
   current Argo CD Application remain unchanged.
4. Reconcile all five login credentials, then prove their Kubernetes Secret type,
   key set, labels, non-empty values, and pairwise distinction without logging
   secret material.
5. Sync the complete console Application. Record every wave and retain the
   topology and migration Job logs and terminal status.
6. Prove API and worker readiness, authenticated role identity, command success,
   audit persistence, denied direct DML for both command roles, and denied DDL or
   RLS bypass for every serving role.
7. Exercise credential rotation and rollback as described below.

Static render evidence is necessary but insufficient. Production activation is
blocked until runtime evidence exists for the exact promoted revision, selected
substrate, database state, secret versions, and workload images.

## Rotation and rejection evidence

Kubernetes does not refresh container environment variables after a Secret
changes. Rotate one login at a time:

1. Update the authoritative secret store and wait for the typed, reload-labeled
   Kubernetes Secret to contain the new version.
2. Wait for CloudNativePG to reconcile the corresponding PostgreSQL login.
3. Restart every consumer deliberately: API and worker for `console_rt`; API only for
   either command login.
4. Wait for rollout/deployment readiness and exercise the affected command path.
5. Prove a new direct database session succeeds as the expected role.
6. Prove the retired password is rejected and retain only redacted evidence.

Do not claim zero-downtime rotation merely because ESO refreshed a Secret or
because Kubernetes completed a restart. Availability requires request-level
evidence over the full rotation window.

## Recovery bundle

The off-cluster recovery bundle must identify, without embedding secret values:

- the exact Git revision, rendered overlay, workload image digests, migration
  ledger state, and CloudNativePG version;
- the authoritative secret-store object/version for `console-db-rt`, both command
  Secrets, and the automatically managed owner credential recovery procedure;
- the expected usernames, Secret names, Secret type, key names, reload label,
  and password-URI encoding rule;
- database backup identity, restore point, restore drill evidence, and the six
  expected role and membership rows;
- topology, migration, readiness, command/audit, denial, retired-credential,
  rotation, and rollback evidence; and
- named operators, custody boundaries, approval record, and incident contacts.

Do not place passwords, URIs containing passwords, OpenBao unseal shares, OCI
Vault values, or root tokens in the bundle.

## Rollback boundary

Rollback is a coordinated application, database, and credential operation. Keep
the expand migrations backward-compatible for the approved rollback window and
retain a compatible application image. Revert the activation reference, sync the
whole Application, restart consumers deliberately, and verify readiness and
command behavior. Revoke new credentials only after the rollback workload no
longer consumes them; then prove rejection.

Do not drop roles, functions, or schema objects as an emergency first step. Do
not delete ExternalSecrets before out-of-band recovery Secrets are ready. A
rollback rehearsal is not production rollback proof unless it used the exact
promoted revision, database state, substrate adapter, and secret versions.

## Render-only checks

```sh
kubectl kustomize deploy/apps/console/overlays/pr-473-expand-on-prem
kubectl kustomize deploy/apps/console/overlays/pr-473-expand-oci-guest
```

These commands must not be piped to `kubectl apply`. The overlays remain DARK
until a later promotion binds one to a reviewed non-production Argo CD
Application. PR 473 must not add this component to `base`, `overlays/prod`, or
the current Argo CD Application.
