#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: ops/backup/backup.sh [options]

Creates a timestamped backup from a running Compose project:
  - Postgres custom-format pg_dump
  - Postgres pg_basebackup PITR base tarball
  - Postgres WAL archive volume snapshot
  - Postgres globals dump
  - SQLx migration metadata and table row counts
  - SeaweedFS /data volume tarball and file manifest

Options:
  --project NAME         Compose project name (default: console-prod)
  --compose-file PATH    Compose file, repeatable (default: ops/compose.yml)
  --backup-root PATH     Backup root (default: ops/backup/artifacts)
  --timestamp VALUE      UTC-ish backup id (default: current YYYYMMDDTHHMMSSZ)
  --force-weekly         Also copy this backup into weekly retention set
  -h, --help             Show this help

Environment overrides:
  COMPOSE_PROJECT_NAME, COMPOSE_FILE, BACKUP_ROOT, BACKUP_TIMESTAMP,
  RETENTION_DAILY (default 14), RETENTION_WEEKLY (default 8)
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
cd "${repo_root}"

compose_project="${COMPOSE_PROJECT_NAME:-console-prod}"
backup_root="${BACKUP_ROOT:-ops/backup/artifacts}"
timestamp="${BACKUP_TIMESTAMP:-$(date -u +%Y%m%dT%H%M%SZ)}"
retention_daily="${RETENTION_DAILY:-14}"
retention_weekly="${RETENTION_WEEKLY:-8}"
force_weekly="${FORCE_WEEKLY:-0}"
compose_files=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      compose_project="$2"
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
    --timestamp)
      timestamp="$2"
      shift 2
      ;;
    --force-weekly)
      force_weekly="1"
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
  "compose-production-backup"

if [[ ${#compose_files[@]} -eq 0 ]]; then
  if [[ -n "${COMPOSE_FILE:-}" ]]; then
    compose_files+=("${COMPOSE_FILE}")
  else
    compose_files+=("ops/compose.yml")
  fi
fi

compose_args=(-p "${compose_project}")
for compose_file in "${compose_files[@]}"; do
  compose_args+=(-f "${compose_file}")
done

compose() {
  docker-compose "${compose_args[@]}" "$@"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 127
  fi
}

sha256_file() {
  local file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file}" | awk '{print $1}'
  else
    shasum -a 256 "${file}" | awk '{print $1}'
  fi
}

service_container() {
  local service="$1"
  local cid
  cid="$(compose ps -q "${service}")"
  if [[ -z "${cid}" ]]; then
    echo "service ${service} is not running in compose project ${compose_project}" >&2
    exit 1
  fi
  printf '%s\n' "${cid}"
}

postgres_exec() {
  compose exec -T postgres sh -ceu "$1" </dev/null
}

postgres_psql() {
  local sql="$1"
  compose exec -T postgres sh -ceu 'PGPASSWORD="${POSTGRES_PASSWORD}" psql -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -v ON_ERROR_STOP=1 -At -c "$1"' psql-wrapper "${sql}" </dev/null
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

prune_backups() {
  local dir="$1"
  local keep="$2"
  [[ -d "${dir}" ]] || return 0
  local index=0
  local entry
  while IFS= read -r entry; do
    [[ -z "${entry}" ]] && continue
    index=$((index + 1))
    if (( index > keep )); then
      rm -rf "${entry}"
      echo "pruned ${entry}"
    fi
  done < <(find "${dir}" -mindepth 1 -maxdepth 1 -type d -print | sort -r)
}

require_cmd docker-compose
require_cmd docker

postgres_cid="$(service_container postgres)"
seaweedfs_cid="$(service_container seaweedfs)"
postgres_wal_archive_volume="$(volume_for_mount "${postgres_cid}" /var/lib/postgresql/wal-archive)"
seaweedfs_volume="$(volume_for_mount "${seaweedfs_cid}" /data)"

backup_dir="${backup_root}/daily/${timestamp}"
if [[ -e "${backup_dir}" ]]; then
  echo "backup directory already exists: ${backup_dir}" >&2
  exit 1
fi
mkdir -p "${backup_dir}"
backup_dir_host="$(host_path "${backup_dir}")"

echo "backup_id=${timestamp}"
echo "compose_project=${compose_project}"
echo "backup_dir=${backup_dir}"

postgres_exec 'pg_isready -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}"'

postgres_exec 'PGPASSWORD="${POSTGRES_PASSWORD}" pg_dump -h 127.0.0.1 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" -Fc' >"${backup_dir}/postgres.dump"
postgres_exec 'PGPASSWORD="${POSTGRES_PASSWORD}" pg_dumpall -h 127.0.0.1 -U "${POSTGRES_USER}" --globals-only' >"${backup_dir}/postgres-globals.sql"
postgres_exec '
  tmp_dir="$(mktemp -d /tmp/console-pitr-base.XXXXXX)"
  cleanup() { rm -rf "${tmp_dir}"; }
  trap cleanup EXIT
  PGPASSWORD="${POSTGRES_PASSWORD}" pg_basebackup \
    -h 127.0.0.1 \
    -U "${POSTGRES_USER}" \
    -D "${tmp_dir}" \
    --format=plain \
    --wal-method=stream \
    --checkpoint=fast
  tar -C "${tmp_dir}" -czf - .
' >"${backup_dir}/postgres-basebackup.tar.gz"

write_sqlx_migrations "${backup_dir}/sqlx-migrations.tsv"
write_row_counts "${backup_dir}/postgres-row-counts.tsv"

docker run --rm \
  --volume "${postgres_wal_archive_volume}:/wal-archive:ro" \
  --volume "${backup_dir_host}:/backup" \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu '
    tar -C /wal-archive -czf /backup/postgres-wal-archive.tar.gz .
  '

docker run --rm \
  --volume "${seaweedfs_volume}:/data:ro" \
  --volume "${backup_dir_host}:/backup" \
  postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56 \
  bash -ceu '
    tar -C /data -czf /backup/seaweedfs-data.tar.gz .
    cd /data
    find . -type f -print0 | sort -z | xargs -0 -r sha256sum > /backup/seaweedfs-manifest.sha256
  '

{
  echo "backup_id=${timestamp}"
  echo "created_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "compose_project=${compose_project}"
  printf 'compose_files='
  printf '%s ' "${compose_files[@]}"
  printf '\n'
  echo "postgres_container=${postgres_cid}"
  echo "postgres_wal_archive_volume=${postgres_wal_archive_volume}"
  echo "seaweedfs_container=${seaweedfs_cid}"
  echo "seaweedfs_volume=${seaweedfs_volume}"
  echo "retention_daily=${retention_daily}"
  echo "retention_weekly=${retention_weekly}"
} >"${backup_dir}/metadata.env"

{
  for artifact in \
    postgres.dump \
    postgres-globals.sql \
    postgres-basebackup.tar.gz \
    postgres-wal-archive.tar.gz \
    sqlx-migrations.tsv \
    postgres-row-counts.tsv \
    seaweedfs-data.tar.gz \
    seaweedfs-manifest.sha256 \
    metadata.env; do
    printf '%s  %s\n' "$(sha256_file "${backup_dir}/${artifact}")" "${artifact}"
  done
} >"${backup_dir}/SHA256SUMS"

if [[ "${force_weekly}" == "1" || "$(date -u +%u)" == "7" ]]; then
  weekly_dir="${backup_root}/weekly/${timestamp}"
  mkdir -p "$(dirname "${weekly_dir}")"
  rm -rf "${weekly_dir}"
  cp -R "${backup_dir}" "${weekly_dir}"
  echo "weekly_copy=${weekly_dir}"
fi

prune_backups "${backup_root}/daily" "${retention_daily}"
prune_backups "${backup_root}/weekly" "${retention_weekly}"

echo "backup_complete=${backup_dir}"
