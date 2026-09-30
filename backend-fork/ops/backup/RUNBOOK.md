# Backup and Restore Runbook

> **POST-PIVOT UNVERIFIED / HOLD:** This runbook is retained historical/operator
> reference. It does not prove that the described production stack, backups,
> credentials, RPO, or restore path exist, and it does not authorize backup or
> restore mutations. The repository currently authorizes zero production
> mutations. Start with the
> [disk-wipe consolidation handoff](../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

## Scope

This runbook covers the T0.9 nightly backup and restore drill for the Compose
production stack in `ops/compose.yml`. It backs up:

- Postgres with `pg_dump -Fc`, plus role/global metadata.
- Postgres `pg_basebackup` PITR base tarball for T0.13 recovery.
- The Postgres WAL archive volume snapshot present at backup time.
- SQLx migration metadata and per-table row counts for restore verification.
- The SeaweedFS `/data` volume as a tarball plus file checksum manifest.

T0.13 adds continuous WAL archiving and arbitrary timestamp PITR. The nightly
`pg_dump` remains the logical safety net; ADR-0015 RPO recovery uses the
`postgres-basebackup.tar.gz` base plus the continuously synced WAL archive.

## Ownership and Cadence

- Owner: on-call operator for the OCI VM.
- Nightly: run `ops/backup/backup.sh` from the repository root on the VM.
- Weekly: the script automatically copies Sunday UTC backups into the weekly
  retention set. Use `--force-weekly` after an exceptional release.
- Restore drill: run after any backup script or Compose storage change, before
  first production data entry, and at least monthly after launch.

Default local retention is 14 daily backups and 8 weekly backups. Production
should sync the backup root to OCI Object Storage with lifecycle rules that
preserve the same minimum retention and protect the bucket with separate
operator credentials.

## Nightly Backup

The stack must already be running. The script intentionally fails instead of
starting production services implicitly.

```sh
docker-compose -p console-prod -f ops/compose.yml up -d postgres seaweedfs
ops/backup/backup.sh --project console-prod --backup-root /var/backups/mnt
```

The backup directory contains:

- `postgres.dump`: custom-format database dump for `pg_restore`.
- `postgres-globals.sql`: global role metadata from `pg_dumpall`.
- `postgres-basebackup.tar.gz`: plain-format `pg_basebackup` data directory
  tarball, created with streamed WAL and a fast checkpoint.
- `postgres-wal-archive.tar.gz`: point-in-time snapshot of the local WAL
  archive volume. Production must still sync the live WAL archive off-VM
  continuously; this tarball is a backup-time checkpoint, not the only WAL copy.
- `sqlx-migrations.tsv`: SQLx migration versions present at backup time.
- `postgres-row-counts.tsv`: row counts for public base tables.
- `seaweedfs-data.tar.gz`: SeaweedFS volume contents.
- `seaweedfs-manifest.sha256`: file checksums from the SeaweedFS volume.
- `SHA256SUMS` and `metadata.env`: artifact checksums and backup metadata.

## PITR Base Backup Handoff

`ops/backup/backup.sh` now writes the PITR base into the normal timestamped
backup directory:

```sh
ops/backup/backup.sh --project console-prod --backup-root /var/backups/mnt
ls /var/backups/mnt/daily/<timestamp>/postgres-basebackup.tar.gz
```

The Compose stack archives WAL into the `postgres-wal-archive` volume. Sync
that archive off the VM at least every five minutes, preserving file names.
Restore uses a base backup plus all archived WAL files through the target time.
Do not claim ADR-0015 RPO compliance from the nightly `pg_dump` alone.

## Restore Drill

Use a scratch project name that cannot collide with production or other workers.
The drill starts only Postgres and SeaweedFS in that scratch project, restores
the latest backup, verifies migration metadata and row counts, verifies the
SeaweedFS file manifest, and tears the scratch project down with volumes.

```sh
ops/backup/restore-drill.sh \
  --scratch-project console-scratch \
  --backup-root /var/backups/mnt
```

Record evidence with a timestamped log:

```sh
mkdir -p ops/backup/drill-logs
ops/backup/restore-drill.sh \
  --scratch-project console-scratch \
  --backup-root /var/backups/mnt \
  2>&1 | tee "ops/backup/drill-logs/$(date -u +%Y%m%dT%H%M%SZ)-restore-drill.log"
```

Required success markers in the log:

- `verify_sqlx_migrations=match`
- `verify_postgres_row_counts=match`
- `verify_seaweedfs_manifest=match`
- `verify_seaweedfs_service=healthy`
- `restore_drill_complete=ok`
- `scratch_teardown=complete project=<scratch-project>`

## PITR Drill Handoff

Use the DR script for arbitrary timestamp recovery. It calls this backup script
to create the base backup, writes controlled before/after rows, restores into a
separate scratch Compose project, replays WAL to the requested timestamp, and
verifies the before row exists while the after row does not:

```sh
ops/dr/pitr-drill.sh \
  --source-project console-prod \
  --scratch-project console-pitr-scratch \
  --backup-root /var/backups/mnt \
  --target-timestamp "2026-06-12T12:34:56Z"
```

Record evidence under `ops/dr/drill-logs/` as described in
`ops/dr/DR-POLICY.md`.

## Manual Restore Procedure

Use this only for an actual incident or a rehearsal where the scratch drill
script is insufficient.

1. Identify the backup directory and verify `SHA256SUMS`.
2. Stop write traffic to the target environment.
3. Start a clean target Compose project:

   ```sh
   docker-compose -p console-restore -f ops/compose.yml down -v --remove-orphans
   docker-compose -p console-restore -f ops/compose.yml up -d postgres seaweedfs
   ```

4. Restore Postgres:

   ```sh
   docker-compose -p console-restore -f ops/compose.yml exec -T postgres \
     sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" dropdb -h 127.0.0.1 -U "${POSTGRES_USER}" --if-exists "${POSTGRES_DB}"'
   docker-compose -p console-restore -f ops/compose.yml exec -T postgres \
     sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" createdb -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}"'
   docker-compose -p console-restore -f ops/compose.yml exec -T postgres \
     sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" pg_restore -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" --exit-on-error --no-owner --role="${POSTGRES_USER}"' \
     < /path/to/postgres.dump
   ```

5. Stop SeaweedFS, replace the `/data` volume from `seaweedfs-data.tar.gz`,
   compare `seaweedfs-manifest.sha256`, then restart SeaweedFS and verify
   health. Compare before restart because SeaweedFS can rewrite local metadata
   files while booting.
6. Compare `sqlx-migrations.tsv` and `postgres-row-counts.tsv` against the
   restored target.
7. Start the remaining services and verify `/readyz` before reopening traffic.

## Failure Modes

- Backup script cannot find a service: the target Compose project is not
  running or the wrong `--project` was used.
- `pg_dump` or `pg_restore` fails: stop the drill and preserve logs; do not
  rotate out the last known-good backup.
- Migration metadata mismatch: restored database is not the same schema state;
  block release and inspect `_sqlx_migrations` before accepting the backup.
- Row-count mismatch: treat as restore failure unless a table was deliberately
  excluded and the runbook was updated in the same change.
- SeaweedFS manifest mismatch: restore did not reproduce object data; inspect
  the volume tarball and do not promote the target.
- Scratch teardown fails: run
  `docker-compose -p <scratch-project> -f ops/compose.yml down -v --remove-orphans`
  before retrying, so the next drill starts from empty volumes.
