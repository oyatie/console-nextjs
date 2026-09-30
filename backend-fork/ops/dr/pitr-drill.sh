#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ops/dr/pitr-drill.sh --target-timestamp RFC3339 [options]

Creates a PITR base backup from a running source Compose project, writes
controlled before/after rows around the supplied recovery target timestamp,
restores into a scratch Compose project, replays archived WAL to the target, and
verifies that the before row exists while the after row does not.

Options:
  --source-project NAME    Source compose project (default: console-prod)
  --scratch-project NAME   Scratch compose project (default: console-pitr-scratch)
  --compose-file PATH      Compose file, repeatable (default: ops/compose.yml)
  --backup-root PATH       Backup root (default: ops/backup/artifacts)
  --backup-timestamp VALUE Backup id (default: current YYYYMMDDTHHMMSSZ-pitr)
  --target-timestamp VALUE Recovery target, UTC RFC3339 seconds (required)
  --keep-scratch           Do not run docker-compose down -v on exit
  -h, --help               Show this help

Environment overrides:
  SOURCE_PROJECT, SCRATCH_PROJECT, COMPOSE_FILE, BACKUP_ROOT,
  BACKUP_TIMESTAMP, PITR_TARGET_TIMESTAMP, KEEP_SCRATCH
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "${repo_root}"

source_project="${SOURCE_PROJECT:-console-prod}"
scratch_project="${SCRATCH_PROJECT:-console-pitr-scratch}"
backup_root="${BACKUP_ROOT:-ops/backup/artifacts}"
backup_timestamp="${BACKUP_TIMESTAMP:-$(date -u +%Y%m%dT%H%M%SZ)-pitr}"
target_timestamp="${PITR_TARGET_TIMESTAMP:-}"
keep_scratch="${KEEP_SCRATCH:-0}"
compose_files=()

if [[ -n "${COMPOSE_FILE:-}" ]]; then
  compose_files+=("${COMPOSE_FILE}")
else
  compose_files+=("ops/compose.yml")
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --source-project)
      source_project="$2"
      shift 2
      ;;
    --scratch-project)
      scratch_project="$2"
      shift 2
      ;;
    --compose-file)
      compose_files+=("$2")
      shift 2
      ;;
    --backup-root)
      backup_root="$2"
      shift 2
      ;;
    --backup-timestamp)
      backup_timestamp="$2"
      shift 2
      ;;
    --target-timestamp)
      target_timestamp="$2"
      shift 2
      ;;
    --keep-scratch)
      keep_scratch="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

if [[ -z "${target_timestamp}" ]]; then
  echo "missing required --target-timestamp" >&2
  usage >&2
  exit 64
fi

if [[ "${source_project}" == "${scratch_project}" ]]; then
  echo "source and scratch compose projects must be distinct" >&2
  exit 64
fi

bash "${script_dir}/../../scripts/require-production-operation-authority.sh" \
  "compose-pitr-drill"

source_compose_args=(-p "${source_project}")
scratch_compose_args=(-p "${scratch_project}")
for compose_file in "${compose_files[@]}"; do
  source_compose_args+=(-f "${compose_file}")
  scratch_compose_args+=(-f "${compose_file}")
done

source_compose() {
  docker-compose "${source_compose_args[@]}" "$@"
}

scratch_compose() {
  docker-compose "${scratch_compose_args[@]}" "$@"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 127
  fi
}

epoch_utc() {
  local value="$1"
  if date -u -d "${value}" +%s >/dev/null 2>&1; then
    date -u -d "${value}" +%s
    return 0
  fi
  date -j -u -f "%Y-%m-%dT%H:%M:%SZ" "${value}" +%s 2>/dev/null
}

format_recovery_target_time() {
  local epoch="$1"
  if date -u -d "@${epoch}" "+%Y-%m-%d %H:%M:%S+00" >/dev/null 2>&1; then
    date -u -d "@${epoch}" "+%Y-%m-%d %H:%M:%S+00"
    return 0
  fi
  date -u -r "${epoch}" "+%Y-%m-%d %H:%M:%S+00"
}

cleanup() {
  local exit_code=$?
  local teardown_code
  if [[ "${keep_scratch}" == "1" ]]; then
    echo "scratch_teardown=skipped project=${scratch_project}"
  else
    if scratch_compose down -v --remove-orphans; then
      echo "scratch_teardown=complete project=${scratch_project}"
    else
      teardown_code=$?
      echo "scratch_teardown=failed project=${scratch_project} exit_code=${teardown_code}" >&2
      if [[ "${exit_code}" == "0" ]]; then
        exit_code="${teardown_code}"
      fi
    fi
  fi
  exit "${exit_code}"
}
trap cleanup EXIT

wait_healthy() {
  local compose_fn="$1"
  local service="$2"
  local tries="${3:-60}"
  local cid status
  for _ in $(seq 1 "${tries}"); do
    cid="$("${compose_fn}" ps -q "${service}" || true)"
    if [[ -n "${cid}" ]]; then
      status="$(docker inspect "${cid}" --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' 2>/dev/null || true)"
      if [[ "${status}" == "healthy" || "${status}" == "running" ]]; then
        echo "${service}_status=${status}"
        return 0
      fi
    fi
    sleep 2
  done
  echo "service ${service} did not become healthy" >&2
  "${compose_fn}" ps >&2 || true
  "${compose_fn}" logs --tail=120 "${service}" >&2 || true
  return 1
}

service_container() {
  local compose_fn="$1"
  local service="$2"
  local cid
  cid="$("${compose_fn}" ps -q "${service}")"
  if [[ -z "${cid}" ]]; then
    echo "service ${service} is not running" >&2
    exit 1
  fi
  printf '%s\n' "${cid}"
}

volume_for_mount() {
  local cid="$1"
  local destination="$2"
  local volume
  volume="$(docker inspect "${cid}" --format "{{range .Mounts}}{{if eq .Destination \"${destination}\"}}{{.Name}}{{end}}{{end}}")"
  if [[ -z "${volume}" ]]; then
    echo "could not find volume mounted at ${destination} for container ${cid}" >&2
    exit 1
  fi
  printf '%s\n' "${volume}"
}

host_path() {
  local path="$1"
  case "${path}" in
    /*) printf '%s\n' "${path}" ;;
    *) printf '%s\n' "${repo_root}/${path}" ;;
  esac
}

postgres_psql_source() {
  local sql="$1"
  source_compose exec -T postgres sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -v ON_ERROR_STOP=1 -At -c "$1"' psql-wrapper "${sql}" </dev/null
}

postgres_psql_scratch() {
  local sql="$1"
  scratch_compose exec -T postgres sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -v ON_ERROR_STOP=1 -At -c "$1"' psql-wrapper "${sql}" </dev/null
}

wait_until_epoch_has_passed() {
  local target_epoch="$1"
  local now remaining
  while :; do
    now="$(date -u +%s)"
    if (( now > target_epoch )); then
      return 0
    fi
    remaining=$((target_epoch - now + 1))
    if (( remaining > 5 )); then
      sleep 5
    else
      sleep "${remaining}"
    fi
  done
}

wait_for_archive_progress() {
  local previous_wal="$1"
  local archived_wal
  for _ in $(seq 1 60); do
    archived_wal="$(postgres_psql_source "SELECT COALESCE(last_archived_wal, '') FROM pg_stat_archiver;")"
    if [[ -n "${archived_wal}" && "${archived_wal}" != "${previous_wal}" ]]; then
      echo "archived_wal=${archived_wal}"
      return 0
    fi
    sleep 1
  done
  echo "WAL archive did not advance after pg_switch_wal()" >&2
  return 1
}

require_cmd docker
require_cmd docker-compose

target_epoch="$(epoch_utc "${target_timestamp}" || true)"
if [[ -z "${target_epoch}" ]]; then
  echo "target timestamp must parse as UTC RFC3339 seconds: ${target_timestamp}" >&2
  exit 64
fi
recovery_target_time="$(format_recovery_target_time "${target_epoch}")"

drill_id="pitr_${backup_timestamp//[^A-Za-z0-9_]/_}"
backup_dir="${backup_root}/daily/${backup_timestamp}"
backup_dir_host="$(host_path "${backup_dir}")"
postgres_image="postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56"

echo "pitr_drill_id=${drill_id}"
echo "source_project=${source_project}"
echo "scratch_project=${scratch_project}"
echo "target_timestamp=${target_timestamp}"
echo "recovery_target_time=${recovery_target_time}"

wait_healthy source_compose postgres
wait_healthy source_compose seaweedfs

backup_args=(
  --project "${source_project}" \
  --backup-root "${backup_root}" \
  --timestamp "${backup_timestamp}"
)
for compose_file in "${compose_files[@]}"; do
  backup_args+=(--compose-file "${compose_file}")
done
ops/backup/backup.sh "${backup_args[@]}"

if [[ ! -f "${backup_dir}/postgres-basebackup.tar.gz" ]]; then
  echo "missing PITR base backup: ${backup_dir}/postgres-basebackup.tar.gz" >&2
  exit 1
fi

now_epoch="$(date -u +%s)"
if (( target_epoch <= now_epoch + 3 )); then
  echo "target timestamp must be at least 4 seconds in the future after base backup creation" >&2
  echo "now_epoch=${now_epoch} target_epoch=${target_epoch}" >&2
  exit 64
fi

postgres_psql_source "CREATE TABLE IF NOT EXISTS pitr_drill_events (drill_id text NOT NULL, label text NOT NULL, note text NOT NULL, created_at timestamptz NOT NULL DEFAULT clock_timestamp(), PRIMARY KEY (drill_id, label));"
before_created_at="$(postgres_psql_source "INSERT INTO pitr_drill_events (drill_id, label, note) VALUES ('${drill_id}', 'before', 'committed before PITR target') RETURNING to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"');")"
echo "before_row_created_at=${before_created_at}"

wait_until_epoch_has_passed "${target_epoch}"

after_created_at="$(postgres_psql_source "INSERT INTO pitr_drill_events (drill_id, label, note) VALUES ('${drill_id}', 'after', 'committed after PITR target') RETURNING to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"');")"
echo "after_row_created_at=${after_created_at}"

archive_before="$(postgres_psql_source "SELECT COALESCE(last_archived_wal, '') FROM pg_stat_archiver;")"
postgres_psql_source "SELECT pg_switch_wal();" >/dev/null
wait_for_archive_progress "${archive_before}"

source_postgres_cid="$(service_container source_compose postgres)"
source_archive_volume="$(volume_for_mount "${source_postgres_cid}" /var/lib/postgresql/wal-archive)"
echo "source_wal_archive_volume=${source_archive_volume}"

scratch_compose down -v --remove-orphans >/dev/null 2>&1 || true
scratch_compose up -d postgres
wait_healthy scratch_compose postgres
scratch_postgres_cid="$(service_container scratch_compose postgres)"
scratch_data_volume="$(volume_for_mount "${scratch_postgres_cid}" /var/lib/postgresql)"
scratch_archive_volume="$(volume_for_mount "${scratch_postgres_cid}" /var/lib/postgresql/wal-archive)"
scratch_pgdata="$(scratch_compose exec -T postgres sh -ceu 'printf "%s\n" "${PGDATA}"' </dev/null)"
case "${scratch_pgdata}" in
  /var/lib/postgresql/*)
    ;;
  *)
    echo "unexpected scratch PGDATA path: ${scratch_pgdata}" >&2
    exit 1
    ;;
esac
echo "scratch_pgdata=${scratch_pgdata}"
scratch_compose stop postgres >/dev/null

docker run --rm \
  --volume "${source_archive_volume}:/source-wal:ro" \
  --volume "${scratch_archive_volume}:/restore-wal" \
  "${postgres_image}" \
  bash -ceu '
    find /restore-wal -mindepth 1 -maxdepth 1 -exec rm -rf {} +
    cp -a /source-wal/. /restore-wal/
    chown -R postgres:postgres /restore-wal
  '

docker run --rm \
  --env "RECOVERY_TARGET_TIME=${recovery_target_time}" \
  --env "PGDATA_TARGET=${scratch_pgdata}" \
  --volume "${scratch_data_volume}:/var/lib/postgresql" \
  --volume "${backup_dir_host}:/backup:ro" \
  "${postgres_image}" \
  bash -ceu '
    case "${PGDATA_TARGET}" in
      /var/lib/postgresql/*) ;;
      *) echo "unexpected PGDATA_TARGET=${PGDATA_TARGET}" >&2; exit 1 ;;
    esac
    rm -rf "${PGDATA_TARGET}"
    mkdir -p "${PGDATA_TARGET}"
    tar -C "${PGDATA_TARGET}" -xzf /backup/postgres-basebackup.tar.gz
    rm -f "${PGDATA_TARGET}/postmaster.pid"
    touch "${PGDATA_TARGET}/recovery.signal"
    quote="$(printf "\047")"
    {
      printf "restore_command = %scp /var/lib/postgresql/wal-archive/%%f %%p%s\n" "${quote}" "${quote}"
      printf "recovery_target_time = %s%s%s\n" "${quote}" "${RECOVERY_TARGET_TIME}" "${quote}"
      printf "recovery_target_timeline = %slatest%s\n" "${quote}" "${quote}"
      printf "recovery_target_action = %spromote%s\n" "${quote}" "${quote}"
    } >> "${PGDATA_TARGET}/postgresql.auto.conf"
    chown -R postgres:postgres "${PGDATA_TARGET}"
  '

scratch_compose up -d postgres
wait_healthy scratch_compose postgres

for _ in $(seq 1 60); do
  in_recovery="$(postgres_psql_scratch "SELECT pg_is_in_recovery();")"
  if [[ "${in_recovery}" == "f" ]]; then
    echo "scratch_recovery=promoted"
    break
  fi
  sleep 2
done

in_recovery="$(postgres_psql_scratch "SELECT pg_is_in_recovery();")"
if [[ "${in_recovery}" != "f" ]]; then
  echo "scratch database did not promote after recovery target" >&2
  exit 1
fi

before_count="$(postgres_psql_scratch "SELECT count(*) FROM pitr_drill_events WHERE drill_id = '${drill_id}' AND label = 'before';")"
after_count="$(postgres_psql_scratch "SELECT count(*) FROM pitr_drill_events WHERE drill_id = '${drill_id}' AND label = 'after';")"

if [[ "${before_count}" != "1" ]]; then
  echo "verify_before_row=failed count=${before_count}" >&2
  exit 1
fi
echo "verify_before_row=exists count=${before_count}"

if [[ "${after_count}" != "0" ]]; then
  echo "verify_after_row=failed count=${after_count}" >&2
  exit 1
fi
echo "verify_after_row=absent count=${after_count}"

echo "pitr_drill_complete=ok"
