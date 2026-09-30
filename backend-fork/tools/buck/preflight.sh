#!/usr/bin/env bash
# Cheap, fail-closed Buck2 graph preflight.  It never writes to the caller's
# worktree: generation runs against a temporary archive of HEAD instead.
set -euo pipefail

generated_face_tier="cheap"
case "${1:-}" in
  "") ;;
  --full-generated-faces) generated_face_tier="all" ;;
  *)
    echo "usage: $0 [--full-generated-faces]" >&2
    exit 2
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
buck_bin="${CONSOLE_BUCK_PREFLIGHT_BUCK:-${repo_root}/tools/buck2}"
expected_release="2026-07-15"
safe_user="${USER:-user}"
safe_user="${safe_user//[^[:alnum:]_.-]/_}"
repo_hash="$(printf '%s' "${repo_root}" | cksum | awk '{print $1}')"
isolation_name="${CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR:-console-buck-preflight-${safe_user}-${repo_hash}}"

if [[ ! "${isolation_name}" =~ ^[[:alnum:]_.-]+$ ]]; then
  echo "buck-preflight: isolation name must contain only letters, digits, dot, underscore, or dash" >&2
  exit 2
fi

manifest="${repo_root}/tools/buck2"
python3 - "${manifest}" "${expected_release}" <<'PY'
import json
import re
import sys
from pathlib import Path

manifest_path = Path(sys.argv[1])
release = sys.argv[2]
contents = manifest_path.read_text(encoding="utf-8").splitlines()
try:
    manifest = json.loads("\n".join(contents[1:]))
except (IndexError, json.JSONDecodeError) as error:
    raise SystemExit(f"buck-preflight: invalid dotslash manifest: {error}")

platforms = manifest.get("platforms")
if not isinstance(platforms, dict) or not platforms:
    raise SystemExit("buck-preflight: dotslash manifest has no platforms")

for platform, entry in platforms.items():
    providers = entry.get("providers")
    urls = [provider.get("url") for provider in providers or []]
    if (
        entry.get("hash") != "blake3"
        or not isinstance(entry.get("size"), int)
        or entry["size"] <= 0
        or not re.fullmatch(r"[0-9a-f]{64}", entry.get("digest", ""))
        or not isinstance(entry.get("path"), str)
        or not entry["path"]
        or not urls
        or any(
            not isinstance(url, str)
            or f"releases/download/{release}/" not in url
            for url in urls
        )
    ):
        raise SystemExit(
            f"buck-preflight: invalid immutable {release} pin for {platform}"
        )
PY

echo "buck-preflight: pinned release ${expected_release}"
BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" --version
BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" audit cell

if ! targets="$(BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" uquery "kind('rust_test', '//backend/...')")"; then
  echo "buck-preflight: rust target enumeration failed" >&2
  exit 1
fi
if ! grep -Eq '(^|[[:space:]])(root)?//backend/' <<<"${targets}"; then
  echo "buck-preflight: no backend rust_test targets were enumerated" >&2
  exit 1
fi
printf 'buck-preflight: enumerated %s Rust test target(s)\n' "$(grep -Ec '(^|[[:space:]])(root)?//backend/' <<<"${targets}")"

scratch="$(python3 "${repo_root}/tools/buck/snapshot_root.py" --repo "${repo_root}")"
baseline="${scratch}/baseline"
archive="${scratch}.tar"
cleanup() {
  local exit_status="$?"
  local cleanup_failed=0
  trap - EXIT HUP INT TERM

  rm -f -- "${archive}" || cleanup_failed=1
  if [[ -e "${scratch}" ]]; then
    python3 "${repo_root}/tools/buck/snapshot_root.py" --repo "${repo_root}" --cleanup "${scratch}" || cleanup_failed=1
  fi

  if (( cleanup_failed )); then
    echo "buck-preflight: failed to clean immutable snapshot" >&2
    return 1
  fi
  return "${exit_status}"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# One immutable archive of the committed candidate is extracted independently
# for the writable candidate and immutable baseline. Never use caller files as
# a drift baseline: they may be dirty and newer than the candidate.
git -C "${repo_root}" archive --format=tar HEAD >"${archive}"
tar -xf "${archive}" -C "${scratch}"
mkdir "${baseline}"
tar -xf "${archive}" -C "${baseline}"
# Validate the registry from the immutable candidate snapshot. This prevents a
# dirty caller tree from supplying roots, writers, or Buck labels that do not
# exist in the candidate we actually gate.
BUCK_ISOLATION_DIR="${isolation_name}" python3 \
  "${scratch}/tools/buck/validate_generated_faces.py" \
  "${scratch}/tools/buck/generated_face_registry.json"
# Archive snapshots deliberately exclude mutable node_modules. Link the caller's
# dependency tree only after package.json/package-lock digest parity and locked
# direct-package validation; no generator can resolve an ambient ancestor tree.
python3 "${repo_root}/tools/buck/provision_snapshot_node_modules.py" \
  --caller "${repo_root}" \
  --snapshot "${scratch}"
# Cheap admission runs only cheap faces and reports every expensive face as a
# visible deferral. --full-generated-faces executes every registered face before
# merge. The runner dispatches only allowlisted writer paths; registry strings
# are never eval'd as shell commands. A non-zero writer or output diff fails
# closed.
python3 "${repo_root}/tools/buck/run_generated_face_gates.py" \
  --registry "${scratch}/tools/buck/generated_face_registry.json" \
  --baseline "${baseline}" \
  --snapshot "${scratch}" \
  --tier "${generated_face_tier}"

echo "buck-preflight: ${generated_face_tier} PASS"
