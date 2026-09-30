#!/usr/bin/env bash
# Decide whether this invocation may use the warm cache, and never fail because
# the cache is down.
#
# WHY: buck2 does NOT degrade when the CAS is unreachable. With a remote-cache
# execution platform selected it queries RE capabilities before running any
# action and treats failure as fatal:
#   Internal error (stage: remote_action_cache): ... Connection refused
#   BUILD FAILED
# So every warm-capable invocation must be guarded by a reachability probe.
# Verified fallback: `buck2 build --no-remote-cache ...` succeeds with the CAS down.
#
# Usage:
#   eval "$(scripts/cas/cas-preflight.sh --endpoint 127.0.0.1:50051)"
#   tools/buck2 --config-file infra/ci/buckconfig/warm-cache.buckconfig \
#       build $BUCK2_CAS_FLAGS //...
#
# Emits shell assignments on stdout and, under GitHub Actions, also writes
# cas_up / flags to $GITHUB_OUTPUT. Always exits 0.
set -uo pipefail

ENDPOINT="${OYA_CAS_ENDPOINT:-127.0.0.1:50051}"
TIMEOUT="${OYA_CAS_PROBE_TIMEOUT:-3}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --endpoint) ENDPOINT="$2"; shift 2 ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 0 ;;
  esac
done

HOST="${ENDPOINT%:*}"
PORT="${ENDPOINT##*:}"

up=0
if command -v nc >/dev/null 2>&1; then
  nc -z -w "$TIMEOUT" "$HOST" "$PORT" >/dev/null 2>&1 && up=1
else
  # bash /dev/tcp fallback; no timeout granularity, best effort
  (exec 3<>"/dev/tcp/$HOST/$PORT") >/dev/null 2>&1 && up=1
fi

if (( up )); then
  FLAGS=""
  echo "# CAS reachable at ${ENDPOINT}: warm cache enabled" >&2
else
  FLAGS="--no-remote-cache"
  echo "# CAS UNREACHABLE at ${ENDPOINT}: falling back to --no-remote-cache" >&2
fi

echo "BUCK2_CAS_FLAGS=\"${FLAGS}\"; export BUCK2_CAS_FLAGS"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  { echo "cas_up=${up}"; echo "flags=${FLAGS}"; } >>"$GITHUB_OUTPUT"
fi
exit 0
