# Operations

> **POST-PIVOT UNVERIFIED / HOLD:** Production topology and operating claims in
> this directory are retained as historical/operator reference and are not
> evidence of a live environment or authority to mutate one. Local development
> instructions remain local-only. The repository currently authorizes zero
> production mutations. Start with the
> [disk-wipe consolidation handoff](../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

This directory contains the Docker Compose production stack for the MNT FSM backend.

## Local Verification

Boot the production stack:

```sh
export CONSOLE_POSTGRES_ADMIN_PASSWORD="$(openssl rand -hex 32)"
export CONSOLE_APP_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
export CONSOLE_RT_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
export CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
export CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
export CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD="$(openssl rand -hex 32)"
docker compose -f ops/compose.yml config --quiet
docker compose -f ops/compose.yml up -d
```

Check the public Traefik HTTPS route:

```sh
curl -k https://mnt.localhost/healthz
curl -k https://mnt.localhost/readyz
```

For direct service access during development, add the override:

```sh
docker compose -f ops/compose.yml -f ops/compose.dev.yml up -d
curl http://127.0.0.1:8080/healthz
```

For the host-launched full-stack developer loop, prefer the npm wrapper:

```sh
npm run dev:up
```

`scripts/dev-up.mjs` layers `ops/compose.dev-deps.yml` on top of
`ops/compose.yml` and `ops/compose.dev.yml`, using the `console-dev` Compose project
by default. It builds exactly one repository-pinned Buck2 app target per run,
then executes that exact `buck-out` artifact for both migrations and the API:
`//backend/app:console-app` normally, or `//backend/app:console-app-dev-auth` only when
`CONSOLE_DEV_AUTH_E2E=1`. It never falls back to a locally compiled backend binary.
The launcher rejects absolute, missing, non-executable, and non-`buck-out`
outputs. Its local PID state also records the child command and OS start token;
`dev:down` signals a process group only if that identity still matches, then
removes stale state rather than risking a reused PID.
The host process derives its platform-force command database URL from the same
local topology password as Compose, replacing any inherited URL rather than
accepting a mismatched capability; neither credential nor URL is persisted.
That dev-only overlay adds Mailpit, a published OTEL port, and the Postgres WAL
archive retention helper described below.

Shut the stack down:

```sh
docker compose -f ops/compose.yml down
```

## Dev Postgres WAL Archive Retention

Issue #366 is guarded by a dev-only WAL archive pruner so repeated
`npm run dev:up` / `npm run dev:down` cycles do not leave an unbounded local WAL
archive volume.

- Where archives live: `ops/compose.yml` still archives Postgres WAL into the
  project-scoped `postgres-wal-archive` volume. In the normal dev wrapper this is
  Docker volume `console-dev_postgres-wal-archive`, mounted at
  `/var/lib/postgresql/wal-archive` in Postgres and `/wal-archive` in the pruner.
- Retention limit: the default policy deletes matching WAL/timeline/backup
  history archive files older than 72 hours and also caps retained archive bytes
  at 1 GiB, while always preserving at least the newest 8 archive files for local
  PITR/debug safety.
- Enforcement: `postgres-wal-archive-pruner` is defined only in
  `ops/compose.dev-deps.yml` and is included by `scripts/dev-up.mjs` for
  `dev:up` and `dev:bootstrap`. It runs outside Postgres `archive_command`, waits
  for the dev Postgres container to become healthy, prunes once every 300
  seconds, and logs as `dev-wal-pruner`. A pruner failure must not make Postgres
  WAL archiving fail.
- Overrides: `CONSOLE_DEV_WAL_ARCHIVE_RETENTION_HOURS`,
  `CONSOLE_DEV_WAL_ARCHIVE_MAX_BYTES`, `CONSOLE_DEV_WAL_ARCHIVE_MIN_SEGMENTS`, and
  `CONSOLE_DEV_WAL_ARCHIVE_PRUNE_INTERVAL_SECONDS` tune the dev policy. Set the age
  or size bound to `0`/`off` only for a temporary local PITR drill that needs
  extra WAL history.

Inspect current local dev WAL archive usage without relying on host-specific
Docker volume mount paths:

```sh
docker run --rm \
  -v console-dev_postgres-wal-archive:/wal-archive:ro \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu 'du -sh /wal-archive; find /wal-archive -maxdepth 1 -type f | wc -l'
```

If local WAL archive storage is already large, first run a one-shot prune through
the same service contract:

```sh
docker compose -p console-dev \
  -f ops/compose.yml -f ops/compose.dev.yml -f ops/compose.dev-deps.yml \
  run --rm -e CONSOLE_DEV_WAL_ARCHIVE_PRUNE_ONCE=1 postgres-wal-archive-pruner --once
```

For an urgent local cleanup where no local PITR/debug archive needs to be kept,
stop the dev stack and remove only the dev WAL archive volume:

```sh
npm run dev:down
docker volume rm console-dev_postgres-wal-archive
```

The next `npm run dev:up` recreates the archive volume and starts the pruner with
the bounded policy. Do not apply this cleanup to production-like Compose projects
or production DR storage; `ops/compose.yml`, `ops/backup/`, and `ops/dr/` remain
responsible for continuous WAL archiving and off-VM retention.

## Image Pins

- Postgres is pinned to `postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56`.
- Traefik v3 was live-verified as `traefik:v3.7.5`.
- SeaweedFS is pinned to `chrislusf/seaweedfs:4.32`, one release behind the live `4.33` line per ADR-0005.
- OpenTelemetry Collector contrib is pinned to `0.154.0`.

Refresh a digest before an architecture change:

```sh
docker manifest inspect postgres:18.4
```

## OCI Deployment Steps

1. Provision an OCI Compute VM with Docker Engine and the Compose plugin installed.
2. Create DNS records for the production host and point them at the VM public IP.
3. Set production environment variables in the VM shell or a root-readable env file:

```sh
export CONSOLE_APP_HOST=api.example.com
export CONSOLE_POSTGRES_DB=console_prod
export CONSOLE_POSTGRES_ADMIN_USER=console_cluster_admin
export CONSOLE_POSTGRES_ADMIN_PASSWORD='<cluster bootstrap administrator password>'
export CONSOLE_APP_POSTGRES_PASSWORD='<migration owner password>'
export CONSOLE_RT_POSTGRES_PASSWORD='<runtime password>'
export CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD='<distinct value from the production secret manager>'
export CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD='<another distinct value from the production secret manager>'
export CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD='<a third distinct command-only value from the production secret manager>'
```

All six passwords are mandatory and pairwise distinct. `postgres` starts with
the cluster administrator, then the one-shot `postgres-topology` service runs
`postgres-reconcile-topology.sh` on both fresh and existing volumes. It creates
or pins the exact seven application roles, makes `console_app` the database/schema
owner, gives that migration-only identity explicit `BYPASSRLS` for populated
tenant-wide backfills, makes it a non-admin member of both NOLOGIN definers,
and verifies readback. The `migrate` service then connects directly as
`console_app`; API and worker connect directly as `console_rt`. Runtime, command, and
definer roles remain `NOBYPASSRLS`. Reconciliation also pins exact serving-role
defaults (`statement_timeout=30s`, `idle_in_transaction_session_timeout=30s`,
and PostgreSQL 17+ `transaction_timeout=45s`), removes only those three keys
from every higher-precedence database-role override, and preserves unrelated
settings. Prepared transactions must remain disabled because they are exempt
from `transaction_timeout`. After the settings transaction commits, existing
serving-role sessions are terminated and fresh direct logins verify the
effective values before migrations may start. These USERSET defaults are a
reconciliation/startup correctness backstop, not a security boundary against a
compromised serving process. They bound normal serving-role transactions only;
migration-owner, offline, and operator writers remain outside this control, so
gap-free sealing still requires quiescence/coordination or a future
xmin/snapshot watermark.

The portable reconciler also rotates role passwords, so it deliberately drains
all existing serving sessions after a successful reconciliation to invalidate
old authenticated sessions. The recurring DARK CloudNativePG Sync hook does
not rotate credentials: it first classifies each role's managed timeout catalog,
repairs and drains only roles with drift, and preserves healthy sessions on an
exact-state whole-Application sync.

An existing volume where `console_app` is a superuser fails closed. After auditing
that volume, choose a new distinct cluster-admin credential and perform the
one-time guarded conversion explicitly. Start only PostgreSQL first: the new
admin does not exist in the old volume yet. The topology container then uses
the shared local socket as the extant `console_app` bootstrap superuser. PostgreSQL
18 does not permit any role to remove `SUPERUSER` from that bootstrap identity,
so the guarded conversion creates a temporary administrator, renames the
bootstrap identity to the requested distinct admin, recreates `console_app` as the
non-superuser migration role, and transfers user-schema ownership to it. Every
password-bearing statement runs with transaction-local logging suppression.

```sh
export CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION=1
docker compose -f ops/compose.yml up -d postgres
docker compose -f ops/compose.yml run --rm postgres-topology
unset CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION
```

The conversion flag must not remain in an environment file. No password belongs
in git, shell tracing, tickets, or logs, and no serving service receives the
cluster-admin or owner URL.

4. Copy the repository checkout to the VM, then build and boot:

```sh
docker compose -f ops/compose.yml up -d --build
docker compose -f ops/compose.yml ps
curl -k "https://${CONSOLE_APP_HOST}/readyz"
```

5. Configure OCI firewall/security-list ingress for `80/tcp` and `443/tcp` only. Do not expose Postgres, SeaweedFS master/filer/admin ports, or the app container directly.
6. Configure backups and PITR under the T0.9/T0.13 runbooks before production data enters the system.

Actual OCI provisioning is an operator action; this repository only declares the deployable stack.
