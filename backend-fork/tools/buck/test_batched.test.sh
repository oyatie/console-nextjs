#!/usr/bin/env bash
# Behavior locks for test_batched.sh; uses a fake Buck binary.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="${repo_root}/tools/buck/test_batched.sh"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/console-buck-batched-test.XXXXXX")"
trap 'rm -rf "${scratch}"' EXIT
log="${scratch}/calls.log"

cat >"${scratch}/buck" <<'BUCK'
#!/usr/bin/env bash
printf 'BUCK_ISOLATION_DIR=%s %s\n' "${BUCK_ISOLATION_DIR:-}" "$*" >>"${HARNESS_LOG}"
case "$1" in
  uquery)
    if [[ "${FAKE_BUCK_MODE:-pass}" != empty ]]; then
      printf 'root//backend/a:unit\nroot//backend/b:unit\n'
    fi
    ;;
  test)
    case "${FAKE_BUCK_MODE:-pass}" in
      pass) printf 'Pass 2. Fail 0.\n' ;;
      fail) printf 'Pass 1. Fail 1.\n'; exit 1 ;;
      crash) printf 'launcher failed\n'; exit 23 ;;
    esac
    ;;
  *) echo "unexpected Buck command: $*" >&2; exit 2 ;;
esac
BUCK
chmod +x "${scratch}/buck"

run() {
  HARNESS_LOG="${log}" CONSOLE_BUCK_TEST_BATCHED_BUCK="${scratch}/buck" \
    CONSOLE_BUCK_TEST_BATCHED_ISOLATION_DIR="batched-lock" "${harness}" 1
}

run
grep -Fq 'BUCK_ISOLATION_DIR=batched-lock uquery ' "${log}"
test "$(grep -Fc 'BUCK_ISOLATION_DIR=batched-lock test ' "${log}")" = 2

if HARNESS_LOG="${log}" CONSOLE_BUCK_TEST_BATCHED_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_TEST_BATCHED_ISOLATION_DIR="batched-lock" FAKE_BUCK_MODE=fail "${harness}" 1; then
  echo "expected failing target batch to fail the harness" >&2
  exit 1
fi
if HARNESS_LOG="${log}" CONSOLE_BUCK_TEST_BATCHED_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_TEST_BATCHED_ISOLATION_DIR="batched-lock" FAKE_BUCK_MODE=crash "${harness}" 1; then
  echo "expected crashed target batch to fail the harness" >&2
  exit 1
fi
if "${harness}" 0 >/dev/null 2>&1; then
  echo "expected invalid batch size to fail" >&2
  exit 1
fi
if HARNESS_LOG="${log}" CONSOLE_BUCK_TEST_BATCHED_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_TEST_BATCHED_ISOLATION_DIR="batched-lock" FAKE_BUCK_MODE=empty "${harness}"; then
  echo "expected an empty target query to fail" >&2
  exit 1
fi

echo "test_batched: PASS"
