#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ops/backup/restore-drill.sh [options]

Restores the latest backup into a scratch Compose project, verifies migration
metadata, Postgres row counts, and SeaweedFS volume file checksums, then tears
the scratch project down by default.

Options:
  --scratch-project NAME  Scratch compose project (default: console-scratch)
  --compose-file PATH     Compose file, repeatable (default: ops/compose.yml)
  --backup-root PATH      Backup root (default: ops/backup/artifacts)
  --backup-dir PATH       Specific backup directory to restore
  --keep-scratch          Do not run docker-compose down -v on exit
  -h, --help              Show this help

Environment overrides:
  SCRATCH_PROJECT, COMPOSE_FILE, BACKUP_ROOT, BACKUP_DIR, KEEP_SCRATCH
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "${repo_root}"

scratch_project="${SCRATCH_PROJECT:-console-scratch}"
backup_root="${BACKUP_ROOT:-ops/backup/artifacts}"
backup_dir="${BACKUP_DIR:-}"
keep_scratch="${KEEP_SCRATCH:-0}"
compose_files=()

if [[ -n "${COMPOSE_FILE:-}" ]]; then
  compose_files+=("${COMPOSE_FILE}")
else
  compose_files+=("ops/compose.yml")
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
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
    --backup-dir)
      backup_dir="$2"
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

bash "${script_dir}/../../scripts/require-production-operation-authority.sh" \
  "compose-restore-drill"

compose_args=(-p "${scratch_project}")
for compose_file in "${compose_files[@]}"; do
  compose_args+=(-f "${compose_file}")
done

compose() {
  docker-compose "${compose_args[@]}" "$@"
}

cleanup() {
  local exit_code=$?
  if [[ "${keep_scratch}" == "1" ]]; then
    echo "scratch_teardown=skipped project=${scratch_project}"
  else
    compose down -v --remove-orphans >/dev/null 2>&1 || true
    echo "scratch_teardown=complete project=${scratch_project}"
  fi
  exit "${exit_code}"
}
trap cleanup EXIT

require_file() {
  if [[ ! -f "$1" ]]; then
    echo "missing required backup artifact: $1" >&2
    exit 1
  fi
}

postgres_exec() {
  compose exec -T postgres sh -ceu "$1" </dev/null
}

postgres_psql() {
  local sql="$1"
  compose exec -T postgres sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -v ON_ERROR_STOP=1 -At -c "$1"' psql-wrapper "${sql}" </dev/null
}

wait_healthy() {
  local service="$1"
  local tries="${2:-60}"
  local cid status
  for _ in $(seq 1 "${tries}"); do
    cid="$(compose ps -q "${service}" || true)"
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
  compose ps >&2 || true
  return 1
}

service_container() {
  local service="$1"
  local cid
  cid="$(compose ps -q "${service}")"
  if [[ -z "${cid}" ]]; then
    echo "service ${service} is not present in scratch project ${scratch_project}" >&2
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

write_sqlx_migrations() {
  local output="$1"
  local has_table
  has_table="$(postgres_psql "SELECT COALESCE(to_regclass('public._sqlx_migrations')::text, '');")"
  printf 'version\tdescription\tsuccess\n' >"${output}"
  if [[ -n "${has_table}" ]]; then
    postgres_exec "PGPASSWORD=\"\${POSTGRES_PASSWORD}\" psql -h 127.0.0.1 -U \"\${POSTGRES_USER}\" -d \"\${POSTGRES_DB}\" -v ON_ERROR_STOP=1 -At -F \"\$(printf '\\t')\" -c 'SELECT version, description, success FROM _sqlx_migrations ORDER BY version;'" >>"${output}"
  fi
}

write_row_counts() {
  local output="$1"
  local tables table count
  printf 'table\trows\n' >"${output}"
  tables="$(postgres_exec "PGPASSWORD=\"\${POSTGRES_PASSWORD}\" psql -h 127.0.0.1 -U \"\${POSTGRES_USER}\" -d \"\${POSTGRES_DB}\" -v ON_ERROR_STOP=1 -At -c \"SELECT format('%I.%I', schemaname, tablename) FROM pg_tables WHERE schemaname = 'public' AND tablename NOT LIKE 'location_pings_%' ORDER BY 1;\"")"
  while IFS= read -r table; do
    [[ -z "${table}" ]] && continue
    count="$(postgres_exec "PGPASSWORD=\"\${POSTGRES_PASSWORD}\" psql -h 127.0.0.1 -U \"\${POSTGRES_USER}\" -d \"\${POSTGRES_DB}\" -v ON_ERROR_STOP=1 -At -c 'SELECT count(*) FROM ${table};'")"
    printf '%s\t%s\n' "${table}" "${count}" >>"${output}"
  done <<<"${tables}"
}

latest_backup_dir() {
  find "${backup_root}/daily" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort | tail -n 1
}

if [[ -z "${backup_dir}" ]]; then
  backup_dir="$(latest_backup_dir)"
fi

if [[ -z "${backup_dir}" || ! -d "${backup_dir}" ]]; then
  echo "no backup directory found under ${backup_root}/daily" >&2
  exit 1
fi
backup_dir_host="$(host_path "${backup_dir}")"

require_file "${backup_dir}/postgres.dump"
require_file "${backup_dir}/sqlx-migrations.tsv"
require_file "${backup_dir}/postgres-row-counts.tsv"
require_file "${backup_dir}/seaweedfs-data.tar.gz"
require_file "${backup_dir}/seaweedfs-manifest.sha256"

scratch_dir="ops/backup/scratch/${scratch_project}"
rm -rf "${scratch_dir}"
mkdir -p "${scratch_dir}"

echo "restore_drill_backup=${backup_dir}"
echo "scratch_project=${scratch_project}"

compose down -v --remove-orphans >/dev/null 2>&1 || true
compose up -d postgres seaweedfs
wait_healthy postgres
wait_healthy seaweedfs

postgres_exec 'PGPASSWORD="${POSTGRES_PASSWORD}" dropdb -h 127.0.0.1 -U "${POSTGRES_USER}" --if-exists "${POSTGRES_DB}"'
postgres_exec 'PGPASSWORD="${POSTGRES_PASSWORD}" createdb -h 127.0.0.1 -U "${POSTGRES_USER}" "${POSTGRES_DB}"'
compose exec -T postgres sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" pg_restore -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" --exit-on-error --no-owner --role="${POSTGRES_USER}"' <"${backup_dir}/postgres.dump"

seaweedfs_cid="$(service_container seaweedfs)"
seaweedfs_volume="$(volume_for_mount "${seaweedfs_cid}" /data)"
compose stop seaweedfs >/dev/null

docker run --rm \
  --volume "${seaweedfs_volume}:/data" \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu 'find /data -mindepth 1 -maxdepth 1 -exec rm -rf {} +'

docker run --rm \
  --volume "${seaweedfs_volume}:/data" \
  --volume "${backup_dir_host}:/backup:ro" \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu 'tar -C /data -xzf /backup/seaweedfs-data.tar.gz'

docker run --rm \
  --volume "${seaweedfs_volume}:/data:ro" \
  --volume "${repo_root}/${scratch_dir}:/scratch" \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu '
    cd /data
    find . -type f -print0 | sort -z | xargs -0 -r sha256sum > /scratch/seaweedfs-manifest.sha256
  '

write_sqlx_migrations "${scratch_dir}/sqlx-migrations.tsv"
write_row_counts "${scratch_dir}/postgres-row-counts.tsv"

diff -u "${backup_dir}/sqlx-migrations.tsv" "${scratch_dir}/sqlx-migrations.tsv"
echo "verify_sqlx_migrations=match"

diff -u "${backup_dir}/postgres-row-counts.tsv" "${scratch_dir}/postgres-row-counts.tsv"
echo "verify_postgres_row_counts=match"

diff -u "${backup_dir}/seaweedfs-manifest.sha256" "${scratch_dir}/seaweedfs-manifest.sha256"
echo "verify_seaweedfs_manifest=match"

compose up -d seaweedfs
wait_healthy seaweedfs
echo "verify_seaweedfs_service=healthy"

echo "restore_drill_complete=ok"
