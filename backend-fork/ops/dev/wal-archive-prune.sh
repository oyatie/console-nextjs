#!/usr/bin/env bash
set -euo pipefail

# DEV-ONLY WAL archive retention helper.
#
# This script runs outside Postgres' archive_command so pruning failures cannot
# make WAL archiving fail. It is mounted by ops/compose.dev-deps.yml only; the
# production-like ops/compose.yml archive/PITR contract is intentionally left
# untouched.

WAL_ARCHIVE_DIR="${CONSOLE_DEV_WAL_ARCHIVE_DIR:-/wal-archive}"
RETENTION_HOURS_RAW="${CONSOLE_DEV_WAL_ARCHIVE_RETENTION_HOURS:-72}"
MAX_BYTES_RAW="${CONSOLE_DEV_WAL_ARCHIVE_MAX_BYTES:-1073741824}"
MIN_SEGMENTS_RAW="${CONSOLE_DEV_WAL_ARCHIVE_MIN_SEGMENTS:-8}"
PRUNE_INTERVAL_SECONDS_RAW="${CONSOLE_DEV_WAL_ARCHIVE_PRUNE_INTERVAL_SECONDS:-300}"

log() {
  printf 'dev-wal-pruner: %s\n' "$*"
}

warn() {
  printf 'dev-wal-pruner: WARN: %s\n' "$*" >&2
}

is_disabled_value() {
  case "${1,,}" in
    0|off|false|disabled|none)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

parse_nonnegative_bound() {
  local name="$1"
  local raw="$2"
  if is_disabled_value "$raw"; then
    printf '0\n'
    return
  fi
  if [[ ! "$raw" =~ ^[0-9]+$ ]]; then
    printf '%s must be a non-negative integer or off/disabled, got %q\n' "$name" "$raw" >&2
    exit 64
  fi
  printf '%s\n' "$raw"
}

parse_positive_int() {
  local name="$1"
  local raw="$2"
  if [[ ! "$raw" =~ ^[0-9]+$ ]] || (( raw < 1 )); then
    printf '%s must be a positive integer, got %q\n' "$name" "$raw" >&2
    exit 64
  fi
  printf '%s\n' "$raw"
}

RETENTION_HOURS="$(parse_nonnegative_bound CONSOLE_DEV_WAL_ARCHIVE_RETENTION_HOURS "$RETENTION_HOURS_RAW")"
MAX_BYTES="$(parse_nonnegative_bound CONSOLE_DEV_WAL_ARCHIVE_MAX_BYTES "$MAX_BYTES_RAW")"
MIN_SEGMENTS="$(parse_nonnegative_bound CONSOLE_DEV_WAL_ARCHIVE_MIN_SEGMENTS "$MIN_SEGMENTS_RAW")"
PRUNE_INTERVAL_SECONDS="$(parse_positive_int CONSOLE_DEV_WAL_ARCHIVE_PRUNE_INTERVAL_SECONDS "$PRUNE_INTERVAL_SECONDS_RAW")"

if [[ ! -d "$WAL_ARCHIVE_DIR" ]]; then
  printf 'WAL archive directory does not exist: %s\n' "$WAL_ARCHIVE_DIR" >&2
  exit 66
fi

# Match files Postgres can place in archive_command destinations: regular WAL
# segment names, timeline history files, and backup history marker files. Do not
# touch temporary files or arbitrary operator notes that may share the volume.
is_archive_file_name() {
  local name="$1"
  [[ "$name" =~ ^[0-9A-F]{24}$ ]] \
    || [[ "$name" =~ ^[0-9A-F]{24}\.[0-9A-F]{8}\.backup$ ]] \
    || [[ "$name" =~ ^[0-9A-F]{8}\.history$ ]]
}

candidate_records() {
  find "$WAL_ARCHIVE_DIR" -maxdepth 1 -type f -printf '%T@\t%s\t%p\n' 2>/dev/null \
    | while IFS=$'\t' read -r mtime bytes file; do
        name="${file##*/}"
        if is_archive_file_name "$name"; then
          printf '%s\t%s\t%s\n' "$mtime" "$bytes" "$file"
        fi
      done \
    | sort -n
}

record_file() {
  local record="$1"
  local _mtime _bytes file
  IFS=$'\t' read -r _mtime _bytes file <<<"$record"
  printf '%s\n' "$file"
}

record_bytes() {
  local record="$1"
  local _mtime bytes _file
  IFS=$'\t' read -r _mtime bytes _file <<<"$record"
  printf '%s\n' "$bytes"
}

record_is_older_than() {
  local record="$1"
  local cutoff_epoch="$2"
  local mtime _bytes _file
  IFS=$'\t' read -r mtime _bytes _file <<<"$record"
  awk -v mtime="$mtime" -v cutoff="$cutoff_epoch" 'BEGIN { exit !(mtime < cutoff) }'
}

prune_by_age() {
  local deleted=0
  if (( RETENTION_HOURS == 0 )); then
    printf '0\n'
    return
  fi

  local count="${#records[@]}"
  local prunable_count=$((count - MIN_SEGMENTS))
  if (( prunable_count <= 0 )); then
    printf '0\n'
    return
  fi

  local now_epoch cutoff_epoch
  now_epoch="$(date +%s)"
  cutoff_epoch=$((now_epoch - (RETENTION_HOURS * 3600)))

  for ((i = 0; i < prunable_count; i++)); do
    local record="${records[$i]}"
    local file
    file="$(record_file "$record")"
    if [[ -e "$file" ]] && record_is_older_than "$record" "$cutoff_epoch"; then
      if rm -f -- "$file"; then
        deleted=$((deleted + 1))
      else
        warn "failed to delete old archive file: $file"
      fi
    fi
  done

  printf '%s\n' "$deleted"
}

prune_by_size() {
  local deleted=0
  if (( MAX_BYTES == 0 )); then
    printf '0\n'
    return
  fi

  local total_bytes=0
  local record bytes
  for record in "${records[@]}"; do
    bytes="$(record_bytes "$record")"
    total_bytes=$((total_bytes + bytes))
  done

  local remaining_count="${#records[@]}"
  local index=0
  while (( total_bytes > MAX_BYTES && remaining_count > MIN_SEGMENTS && index < ${#records[@]} )); do
    record="${records[$index]}"
    bytes="$(record_bytes "$record")"
    local file
    file="$(record_file "$record")"
    if [[ -e "$file" ]]; then
      if rm -f -- "$file"; then
        deleted=$((deleted + 1))
        total_bytes=$((total_bytes - bytes))
        remaining_count=$((remaining_count - 1))
      else
        warn "failed to delete archive file for size cap: $file"
      fi
    fi
    index=$((index + 1))
  done

  if (( total_bytes > MAX_BYTES && remaining_count <= MIN_SEGMENTS )); then
    warn "archive still exceeds max bytes (${total_bytes} > ${MAX_BYTES}) because CONSOLE_DEV_WAL_ARCHIVE_MIN_SEGMENTS=${MIN_SEGMENTS} protects the newest files"
  fi

  printf '%s\n' "$deleted"
}

prune_once() {
  local -a records
  mapfile -t records < <(candidate_records)

  local deleted_age
  deleted_age="$(prune_by_age)"

  mapfile -t records < <(candidate_records)
  local deleted_size
  deleted_size="$(prune_by_size)"

  if (( deleted_age + deleted_size > 0 )); then
    log "deleted ${deleted_age} file(s) by age and ${deleted_size} file(s) by size cap"
  fi
}

log "active for ${WAL_ARCHIVE_DIR}: retention_hours=${RETENTION_HOURS}, max_bytes=${MAX_BYTES}, min_segments=${MIN_SEGMENTS}, interval_seconds=${PRUNE_INTERVAL_SECONDS}"
if (( RETENTION_HOURS == 0 && MAX_BYTES == 0 )); then
  warn "both age and size retention are disabled; use only for local PITR drills"
fi

if [[ "${CONSOLE_DEV_WAL_ARCHIVE_PRUNE_ONCE:-}" == "1" || "${1:-}" == "--once" ]]; then
  prune_once
  exit 0
fi

while true; do
  prune_once || warn "prune pass failed; continuing so Postgres archiving is not affected"
  sleep "$PRUNE_INTERVAL_SECONDS"
done
