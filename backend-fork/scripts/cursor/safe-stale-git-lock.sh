#!/usr/bin/env bash
# Remove a git lock ONLY when it is demonstrably stale.
# Never kill git processes. Never rm locks held by a live PID.
#
# Usage:
#   bash scripts/cursor/safe-stale-git-lock.sh [--min-age-sec N] <lock-path>
#
# Policy (process.git-pkill-lock-race):
#   1. Path must be a git lock (index.lock, HEAD.lock, *.lock under .git/, or gc.pid).
#   2. If the lock body contains a PID and that PID is alive → refuse.
#   3. If lock mtime age < min-age-sec (default 300) → refuse (wait, don't race).
#   4. Otherwise remove the lock file and print what was done.
set -euo pipefail

MIN_AGE=300
LOCK=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --min-age-sec)
      MIN_AGE="${2:?--min-age-sec requires seconds}"
      shift 2
      ;;
    -h|--help)
      sed -n '2,16p' "$0"
      exit 0
      ;;
    -*)
      echo "unknown flag: $1" >&2
      exit 2
      ;;
    *)
      if [[ -n "$LOCK" ]]; then
        echo "unexpected extra arg: $1" >&2
        exit 2
      fi
      LOCK="$1"
      shift
      ;;
  esac
done

if [[ -z "$LOCK" ]]; then
  echo "usage: $0 [--min-age-sec N] <lock-path>" >&2
  exit 2
fi

# Resolve to absolute for reporting; do not require the file to exist yet for path checks.
case "$LOCK" in
  /*) ABS="$LOCK" ;;
  *) ABS="$(pwd)/$LOCK" ;;
esac

base="$(basename "$ABS")"
if [[ "$base" != *.lock && "$base" != "gc.pid" ]]; then
  echo "refuse: not a git lock name ($base)" >&2
  exit 3
fi

# Must live under a .git directory (hub .git or worktree gitdir).
if [[ "$ABS" != */.git/* ]]; then
  echo "refuse: lock path is not under .git/ ($ABS)" >&2
  exit 3
fi

if [[ ! -e "$ABS" ]]; then
  echo "ok: no lock present at $ABS"
  exit 0
fi

now="$(date +%s)"
mtime="$(stat -f %m "$ABS" 2>/dev/null || stat -c %Y "$ABS")"
age=$((now - mtime))
if (( age < MIN_AGE )); then
  echo "refuse: lock age ${age}s < min-age ${MIN_AGE}s — wait; do not pkill git ($ABS)" >&2
  exit 4
fi

# Git lock files are often empty; some hold "PID\n". Treat any leading integer as holder.
holder=""
if [[ -f "$ABS" ]]; then
  holder="$(head -c 64 "$ABS" 2>/dev/null | tr -dc '0-9' | head -c 16 || true)"
fi
if [[ -n "$holder" ]] && kill -0 "$holder" 2>/dev/null; then
  echo "refuse: lock holder PID $holder is alive — wait/timeout; do not kill ($ABS)" >&2
  exit 5
fi

rm -f "$ABS"
echo "removed stale lock age=${age}s holder=${holder:-none} path=$ABS"
