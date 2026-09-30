#!/usr/bin/env bash
# Preflight before forge mutation (gh pr merge, thread resolve storms, push to PR).
# Fail closed: exit 1 prints reason on stderr.
set -euo pipefail

fail() { echo "preflight-forge: $*" >&2; exit 1; }

if ! command -v gh >/dev/null 2>&1; then
  fail "gh not on PATH"
fi

# Auth must work (not merely 'gh' present)
if ! gh auth status -h github.com >/dev/null 2>&1; then
  fail "gh auth broken — run: gh auth login -h github.com (token invalid/expired)"
fi

# Cheap rate-limit probe (authenticated)
if ! gh api rate_limit --jq '.resources.core.remaining' >/dev/null 2>&1; then
  fail "gh api probe failed (auth or network)"
fi
REMAIN="$(gh api rate_limit --jq '.resources.core.remaining' 2>/dev/null || echo 0)"
if [[ "${REMAIN}" =~ ^[0-9]+$ ]] && (( REMAIN < 50 )); then
  fail "GitHub API remaining=${REMAIN} (<50) — wait or re-auth; do not dispatch merge agents"
fi

echo "preflight-forge: ok (remaining≈${REMAIN})"
