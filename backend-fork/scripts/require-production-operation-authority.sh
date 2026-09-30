#!/usr/bin/env bash
set -euo pipefail

operation="${1:-unspecified-production-operation}"

cat >&2 <<EOF
production_operation_authority=blocked
operation=${operation}
reason=the post-pivot repository authorizes zero production mutations
handoff=docs/handoffs/2026-08-03-disk-wipe-consolidation.md
activation=a future candidate-bound, reviewed activation must deliberately replace this hard-fail guard
EOF

exit 78
