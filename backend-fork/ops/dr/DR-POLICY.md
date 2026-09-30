# Disaster Recovery Policy

> **POST-PIVOT UNVERIFIED / HOLD:** This pre-pivot policy is retained as
> historical/operator reference, not as evidence of current production data,
> topology, RPO/RTO attainment, drill status, or activation authority. The
> repository currently authorizes zero production mutations. Start with the
> [disk-wipe consolidation handoff](../../docs/handoffs/2026-08-03-disk-wipe-consolidation.md).

## Scope

This policy covers the single-VM Docker Compose production deployment declared
in `ops/compose.yml`. It is the operating contract for Postgres durability,
point-in-time recovery, and the emergency-dispatch fallback required by
ADR-0015.

Future ADR-0024 multicluster ApplicationSet failover is tracked separately in
`ops/dr/multicluster-failover-runbook.md`. That runbook is DARK until federation,
standby data readiness, and traffic ownership are explicitly activated; it does
not change this single-VM recovery policy.

## Targets

- RPO: <= 5 minutes for Postgres data protected by continuous WAL archiving.
- RTO: <= 1 hour to restore service from a valid base backup plus archived WAL.
- Drill cadence: run the PITR drill after any Compose storage or backup-script
  change, before production data entry, and at least monthly after launch.

Nightly `pg_dump` artifacts remain useful for logical restore and inspection.
They do not satisfy the RPO target by themselves.

## Postgres PITR Design

- `ops/compose.yml` starts Postgres with `archive_mode=on`,
  `wal_level=replica`, and `archive_timeout=300s`.
- WAL files are archived to the project-scoped `postgres-wal-archive` volume.
- `ops/backup/backup.sh` creates `postgres-basebackup.tar.gz` using
  `pg_basebackup --wal-method=stream --checkpoint=fast`.
- Production operations must continuously sync the WAL archive volume off the
  VM at least every five minutes, preserving original WAL file names.
- Recovery restores a base backup into a clean Postgres data directory, provides
  archived WAL through `restore_command`, sets `recovery_target_time`, and
  promotes at the target.

## PITR Drill

Choose a target timestamp at least 90 seconds in the future so the script can
create a base backup, commit a before row, cross the target, commit an after
row, and archive the WAL:

```sh
ops/dr/pitr-drill.sh \
  --source-project console-prod \
  --scratch-project console-pitr-scratch \
  --backup-root /var/backups/mnt \
  --target-timestamp "2026-06-12T12:34:56Z" \
  2>&1 | tee "ops/dr/drill-logs/$(date -u +%Y%m%dT%H%M%SZ)-pitr-drill.log"
```

Required success markers:

- `verify_before_row=exists count=1`
- `verify_after_row=absent count=0`
- `scratch_recovery=promoted`
- `pitr_drill_complete=ok`
- `scratch_teardown=complete project=<scratch-project>`

The source project and scratch project must be distinct. The scratch project is
torn down with volumes unless `--keep-scratch` is set for incident debugging.

## Incident Recovery Procedure

1. Declare incident ownership: 관리자 is incident commander, 접수자 is dispatch
   coordinator.
2. Freeze production writes if the VM is partially available.
3. Select the latest valid base backup before the desired target time.
4. Collect all archived WAL files from the base backup start through the target.
5. Restore into a clean Compose project or replacement VM.
6. Verify `pg_is_in_recovery() = false` after promotion, then verify `/readyz`.
7. Run the row-count and migration checks from `ops/backup/restore-drill.sh`
   when restoring a full production backup set.
8. Reopen traffic only after 관리자 accepts the recovery evidence.

## Degraded Dispatch

If the VM is down while a P1 is in flight, execute
`ops/dr/VM-DOWN-RUNBOOK.md`. The dispatch fallback prioritizes time to first
contact over system-of-record completeness; the recovery step reconciles manual
actions into the audit trail after service returns.
