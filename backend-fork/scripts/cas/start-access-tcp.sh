#!/usr/bin/env bash
# Start Cloudflare Access TCP forwarders for shared CAS (GHA / local remote path).
# Preferred: dockerized cloudflared with --add-host (see ~/oyatie-cas/canary/reapi-access-canary.sh).
# This wrapper calls that canary's forwarder pattern without running the full REAPI probe.
#
# Env (from ~/.env or GHA secrets — never print):
#   CF_ACCESS_WRITE_CLIENT_ID / CF_ACCESS_WRITE_CLIENT_SECRET
#   CF_ACCESS_READ_CLIENT_ID / CF_ACCESS_READ_CLIENT_SECRET
# Or lab files: ~/oyatie-cas/secrets/oyatie-cas-{write,read}.env
set -euo pipefail

LAB="${OYA_CAS_LAB:-$HOME/oyatie-cas}"
if [[ -x "$LAB/canary/reapi-access-canary.sh" ]]; then
  echo "Delegating Access TCP + REAPI probe to lab canary (GREEN_REAPI gate)."
  echo "For forwarders-only, set OYA_CAS_FORWARDERS_ONLY=1 once the lab adds that mode."
  exec "$LAB/canary/reapi-access-canary.sh"
fi

echo "missing lab canary at $LAB/canary/reapi-access-canary.sh" >&2
exit 4
