#!/usr/bin/env bash
# Run the hermetic (non-needs-postgres) rust_test targets under `buck2 test` in
# small batches.  A batch failure is intentionally terminal: batching reports
# every independent failure, it must never turn a red target into a green job.
# Usage: tools/buck/test_batched.sh [BATCH_SIZE]
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo_root}"
BATCH="${1:-4}"
if ! [[ "${BATCH}" =~ ^[1-9][0-9]*$ ]]; then
  echo "buck-batched: batch size must be a positive integer" >&2
  exit 2
fi

# Product verification is repository-pinned.  The override is only a test seam
# for this shell harness; CI and developers use tools/buck2 by default.
buck_bin="${CONSOLE_BUCK_TEST_BATCHED_BUCK:-${repo_root}/tools/buck2}"
safe_user="${USER:-user}"
safe_user="${safe_user//[^[:alnum:]_.-]/_}"
repo_hash="$(printf '%s' "${repo_root}" | cksum | awk '{print $1}')"
isolation_name="${CONSOLE_BUCK_TEST_BATCHED_ISOLATION_DIR:-console-buck-hermetic-${safe_user}-${repo_hash}}"
if [[ ! "${isolation_name}" =~ ^[[:alnum:]_.-]+$ ]]; then
  echo "buck-batched: isolation name must contain only letters, digits, dot, underscore, or dash" >&2
  exit 2
fi

# bash 3.2 (macOS) has no mapfile; target names have no spaces so word-split is safe.
if ! target_output="$(BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" uquery \
  "kind('rust_test', '//backend/...') - attrfilter('labels', 'needs-postgres', '//backend/...')" \
  )"; then
  echo "buck-batched: target enumeration failed" >&2
  exit 1
fi
TARGETS=( $(printf '%s\n' "${target_output}" | sed 's#^root##') )
if [[ ${#TARGETS[@]} -eq 0 ]]; then
  echo "buck-batched: no hermetic Rust test targets were enumerated" >&2
  exit 1
fi
echo "hermetic rust_test targets: ${#TARGETS[@]} (batch size $BATCH)"
total_pass=0 total_fail=0 crashed=()
for ((i = 0; i < ${#TARGETS[@]}; i += BATCH)); do
  group=("${TARGETS[@]:i:BATCH}")
  if out="$(BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" test "${group[@]}" 2>&1 | perl -pe 's/\e\[[0-9;]*m//g')"; then
    command_status=0
  else
    command_status=$?
  fi
  line=$(grep -oE "Pass [0-9]+\. Fail [0-9]+" <<<"$out" | tail -1)
  if [[ -z "$line" ]]; then
    crashed+=("${group[*]}")
    echo "  CRASH/none: ${group[*]##*:}"
  else
    p=$(grep -oE "Pass [0-9]+" <<<"$line"); p=${p#Pass }
    f=$(grep -oE "Fail [0-9]+" <<<"$line"); f=${f#Fail }
    total_pass=$((total_pass + p)); total_fail=$((total_fail + f))
    [[ "$f" != 0 ]] && echo "  FAIL($f): ${group[*]##*:}"
    if [[ "${command_status}" -ne 0 && "${f}" -eq 0 ]]; then
      crashed+=("${group[*]}")
      echo "  CRASH(exit ${command_status}): ${group[*]##*:}"
    fi
  fi
done
echo "=================================================="
echo "TOTAL: Pass $total_pass  Fail $total_fail  CrashedBatches ${#crashed[@]}"
[[ ${#crashed[@]} -gt 0 ]] && printf 'crashed batch: %s\n' "${crashed[@]}"
if [[ "${total_fail}" -gt 0 || ${#crashed[@]} -gt 0 ]]; then
  exit 1
fi
