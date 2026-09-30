#!/usr/bin/env bash
# Fail if a local admission/* tip is ahead of origin and unpublished.
# Failure class: process.admit-tip-unpublished
set -euo pipefail
REPO="${1:-.}"
cd "$REPO"
git fetch origin --quiet 2>/dev/null || true

branch="$(git rev-parse --abbrev-ref HEAD)"
case "$branch" in
  admission/*) ;;
  *)
    echo "check-admit-sync: skip (not on admission/*; on $branch)"
    exit 0
    ;;
esac

local_tip="$(git rev-parse HEAD)"
remote_ref="origin/$branch"
if ! git rev-parse --verify "$remote_ref" >/dev/null 2>&1; then
  echo "check-admit-sync: FAIL — $branch has no origin tip; local $(git rev-parse --short HEAD) unpublished"
  exit 1
fi
remote_tip="$(git rev-parse "$remote_ref")"
if [[ "$local_tip" == "$remote_tip" ]]; then
  echo "check-admit-sync: ok — $branch @ $(git rev-parse --short HEAD) matches origin"
  exit 0
fi
ahead="$(git rev-list --count "$remote_tip..$local_tip" 2>/dev/null || echo '?')"
behind="$(git rev-list --count "$local_tip..$remote_tip" 2>/dev/null || echo '?')"
echo "check-admit-sync: FAIL — process.admit-tip-unpublished"
echo "  local  $(git rev-parse --short "$local_tip") ($ahead ahead)"
echo "  origin $(git rev-parse --short "$remote_tip") ($behind behind)"
echo "  action: force-with-lease push (after C/T legality + local fmt/gate pins), do not babysit idle"
exit 1
