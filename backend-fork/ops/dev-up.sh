#!/usr/bin/env bash
# Superseded by scripts/dev-up.mjs (Node, Windows-portable) — kept as a thin
# delegating shim so `ops/dev-up.sh` still works, and so there is exactly one
# bring-up path instead of two parallel ones. See scripts/dev-up.mjs for the
# real implementation: it runs console-app on the host under bacon (not in a
# container), which is why it also supersedes this script's docker-compose
# `up -d --build app` step.
set -euo pipefail
cd "$(dirname "$0")/.."
exec node scripts/dev-up.mjs "${1:-up}"
