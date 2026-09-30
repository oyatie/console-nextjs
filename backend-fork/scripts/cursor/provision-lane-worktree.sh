#!/usr/bin/env bash
# Provision a lane/admission git worktree UNDER the hub repo (.worktrees/<name>).
# Sibling checkouts (../console-lane-*) sit outside the Cursor workspace root and
# trigger External-File Protection ("allow edit") on every agent write — do not use them.
#
# Usage:
#   bash scripts/cursor/provision-lane-worktree.sh [--hub PATH] [--kind lane|admission] <id>
#
# Examples:
#   bash scripts/cursor/provision-lane-worktree.sh console-ann
#     -> <hub>/.worktrees/lane-console-ann  branch lane/console-ann
#   bash scripts/cursor/provision-lane-worktree.sh --kind admission train2
#     -> <hub>/.worktrees/admission-train2  branch admission/train2
set -euo pipefail

HUB=""
KIND="lane"
ID=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --hub)
      HUB="${2:?--hub requires a path}"
      shift 2
      ;;
    --kind)
      KIND="${2:?--kind requires lane|admission}"
      shift 2
      ;;
    -h|--help)
      sed -n '2,16p' "$0"
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown flag: $1" >&2
      exit 2
      ;;
    *)
      if [[ -n "$ID" ]]; then
        echo "unexpected extra arg: $1" >&2
        exit 2
      fi
      ID="$1"
      shift
      ;;
  esac
done

if [[ -z "$ID" ]]; then
  echo "usage: $0 [--hub PATH] [--kind lane|admission] <id>" >&2
  exit 2
fi

case "$KIND" in
  lane|admission) ;;
  *)
    echo "--kind must be lane or admission (got: $KIND)" >&2
    exit 2
    ;;
esac

# Reject path separators / traversal in the bead id.
if [[ "$ID" == *"/"* || "$ID" == *".."* || "$ID" == *\\* ]]; then
  echo "id must be a single path segment (got: $ID)" >&2
  exit 2
fi

if [[ -z "$HUB" ]]; then
  HUB="$(cd "$(dirname "$0")/../.." && pwd)"
fi
HUB="$(cd "$HUB" && pwd)"

SAFE_ID="${ID//\//-}"
DIR_NAME="${KIND}-${SAFE_ID}"
WT_PATH="${HUB}/.worktrees/${DIR_NAME}"
BRANCH="${KIND}/${ID}"

mkdir -p "${HUB}/.worktrees"

if [[ -e "$WT_PATH" ]]; then
  echo "worktree path already exists: $WT_PATH" >&2
  exit 1
fi

# Match the git-lock-enforcer allowlist exactly (no fetch here — caller may fetch).
git -C "$HUB" worktree add "$WT_PATH" -b "$BRANCH" origin/main

echo "$WT_PATH"
