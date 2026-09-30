#!/usr/bin/env bash
# Behavior and documentation locks for the cheap Buck2 preflight.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="${repo_root}/tools/buck/preflight.sh"
playbook="${repo_root}/docs/program/console-buck2-scale-playbook.md"
roadmap="${repo_root}/docs/program/console-enterprise-roadmap.md"
ledger="${repo_root}/docs/program/console-program-ledger.md"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/console-buck-preflight-test.XXXXXX")"
trap 'rm -rf "${scratch}"' EXIT
mkdir -p "${scratch}/bin" "${scratch}/archive"
log="${scratch}/calls.log"
snapshot="${scratch}/archive"

cat >"${scratch}/buck" <<'BUCK'
#!/usr/bin/env bash
printf 'BUCK_ISOLATION_DIR=%s %s\n' "${BUCK_ISOLATION_DIR:-}" "$*" >>"${HARNESS_LOG}"
case "$1" in
  --version) echo 'buck2 fake' ;;
  audit)
    test "$2" = cell
    echo 'root: /fake/root'
    ;;
  uquery) echo 'root//backend/crates/example:example-unit' ;;
  *) echo "unexpected Buck command: $*" >&2; exit 2 ;;
esac
BUCK
cat >"${scratch}/bin/git" <<'GIT'
#!/usr/bin/env bash
if [[ "$*" == *" archive "* ]]; then
  printf 'GIT_ARCHIVE=%s\n' "$*" >>"${HARNESS_LOG}"
fi
exec "${REAL_GIT}" "$@"
GIT
cat >"${scratch}/bin/python3" <<'PYTHON'
#!/usr/bin/env bash
# The manifest validator runs in the caller worktree; preserve the real
# interpreter there. Only the archive-local generator is substituted.
if [[ "$1" == "-" ]]; then
  exec "${REAL_PYTHON3}" "$@"
fi
if [[ "$1" == */validate_generated_faces.py ]]; then
  printf 'VALIDATE_GENERATED_FACES=%s\n' "$*" >>"${HARNESS_LOG}"
  echo 'generated-face-registry: PASS'
  exit 0
fi
if [[ "$1" == */snapshot_root.py ]]; then
  if [[ "$*" == *"--cleanup"* ]]; then
    printf 'SNAPSHOT_CLEANUP=%s\n' "$*" >>"${HARNESS_LOG}"
    rm -rf "${FAKE_SNAPSHOT_ROOT}"
    exit 0
  fi
  mkdir -p "${FAKE_SNAPSHOT_ROOT}"
  printf '%s\n' "${FAKE_SNAPSHOT_ROOT}"
  exit 0
fi
if [[ "$1" == */provision_snapshot_node_modules.py ]]; then
  printf 'SNAPSHOT_NODE_DEPS=%s\n' "$*" >>"${HARNESS_LOG}"
  if [[ "${FAKE_SNAPSHOT_NODE_DEPS_FAIL:-0}" == 1 ]]; then
    exit 19
  fi
  for relative in tools/buck/preflight.sh tools/buck/generated_face_registry.json web/package.json; do
    baseline_file="${FAKE_SNAPSHOT_ROOT}/baseline/${relative}"
    candidate_file="${FAKE_SNAPSHOT_ROOT}/${relative}"
    test -f "${baseline_file}"
    test -f "${candidate_file}"
    cmp -s "${baseline_file}" "${candidate_file}"
    ! test "${baseline_file}" -ef "${candidate_file}"
  done
  printf '# candidate-only mutation probe\n' >>"${FAKE_SNAPSHOT_ROOT}/.preflight-test-probe"
  test ! -e "${FAKE_SNAPSHOT_ROOT}/baseline/.preflight-test-probe"
  rm -f "${FAKE_SNAPSHOT_ROOT}/.preflight-test-probe"
  mkdir -p "${FAKE_SNAPSHOT_ROOT}/node_modules"
  test ! -e "${FAKE_SNAPSHOT_ROOT}/baseline/node_modules"
  printf 'SNAPSHOT_TREE_INDEPENDENT=1\n' >>"${HARNESS_LOG}"
  exit 0
fi
if [[ "$1" == */run_generated_face_gates.py ]]; then
  printf 'GENERATED_FACE_GATES=%s\n' "$*" >>"${HARNESS_LOG}"
  if [[ "${FAKE_GENERATED_FACE_GATE_SIGNAL_PARENT:-0}" == 1 ]]; then
    kill -TERM "${PPID}"
    exit 75
  fi
  if [[ "${FAKE_STALE_CANDIDATE_DIRTY_CALLER:-0}" == 1 ]]; then
    for ((index = 1; index <= $#; index++)); do
      if [[ "${!index}" == "--baseline" ]]; then
        next=$((index + 1))
        # A caller baseline would mask the simulated stale candidate output;
        # an immutable candidate baseline must expose it as drift.
        [[ "${!next}" == "${PWD}" ]] && exit 0
        exit 23
      fi
    done
    exit 24
  fi
  if [[ "${FAKE_GENERATED_FACE_GATE_FAIL:-0}" == 1 ]]; then
    exit 17
  fi
  exit 0
fi
if [[ "${FAKE_GENERATOR_DRIFT:-0}" == 1 ]]; then
  printf '# drift\n' >> backend/app/BUCK
fi
PYTHON
chmod +x "${scratch}/buck" "${scratch}/bin/python3"
chmod +x "${scratch}/bin/git"

real_python="$(command -v python3)"
real_git="$(command -v git)"
before="$(git -C "${repo_root}" status --porcelain)"
PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" "${harness}"
after="$(git -C "${repo_root}" status --porcelain)"
test "${before}" = "${after}"
grep -Fq 'BUCK_ISOLATION_DIR=preflight-lock --version' "${log}"
grep -Fq 'BUCK_ISOLATION_DIR=preflight-lock audit cell' "${log}"
grep -Fq 'BUCK_ISOLATION_DIR=preflight-lock uquery ' "${log}"
grep -Fq 'SNAPSHOT_NODE_DEPS=' "${log}"
grep -Fq 'SNAPSHOT_TREE_INDEPENDENT=1' "${log}"
grep -Fq 'GENERATED_FACE_GATES=' "${log}"
grep -Fq -- '--tier cheap' "${log}"
grep -Fq "VALIDATE_GENERATED_FACES=${snapshot}/tools/buck/validate_generated_faces.py ${snapshot}/tools/buck/generated_face_registry.json" "${log}"
grep -Fq -- "--registry ${snapshot}/tools/buck/generated_face_registry.json" "${log}"
grep -Fq -- "--baseline ${snapshot}/baseline" "${log}"
test "$(grep -Fc 'GIT_ARCHIVE=' "${log}")" -eq 1
test "$(grep -Fc 'SNAPSHOT_NODE_DEPS=' "${log}")" -eq 1
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

# The runner is the sole writer dispatcher. Preflight must never invoke a
# generator after snapshot verification, which would mutate the caller tree.
if grep -Eq '^tools/buck/generated-face-[a-z-]+\.sh$' "${harness}"; then
  echo "preflight must not invoke generated-face writers outside the snapshot gate" >&2
  exit 1
fi

# Full candidate closure is separately callable and never treats the expensive
# registry faces as an implicit omission.
rm -rf "${snapshot}/baseline"
PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" "${harness}" --full-generated-faces
grep -Fq -- '--tier all' "${log}"
test "$(grep -Fc 'GIT_ARCHIVE=' "${log}")" -eq 2
test "$(grep -Fc 'SNAPSHOT_NODE_DEPS=' "${log}")" -eq 2
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

rm -rf "${snapshot}/baseline"
if PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" FAKE_GENERATED_FACE_GATE_FAIL=1 "${harness}"; then
  echo "expected a registered generated-face gate failure to fail preflight" >&2
  exit 1
fi
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

# A fresh, dirty caller output must not mask stale committed candidate output.
# The fake gate returns success for the old caller baseline and a drift failure
# for the required immutable candidate baseline.
rm -rf "${snapshot}/baseline"
if PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" FAKE_STALE_CANDIDATE_DIRTY_CALLER=1 "${harness}"; then
  echo "expected stale candidate output to fail despite a fresh dirty caller output" >&2
  exit 1
fi
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

rm -rf "${snapshot}/baseline"
if PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" FAKE_SNAPSHOT_NODE_DEPS_FAIL=1 "${harness}"; then
  echo "expected missing or inconsistent snapshot dependencies to fail preflight" >&2
  exit 1
fi
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

# A signal while the generated-face child is active must still clean both the
# archive and both extracted trees through the preflight EXIT trap.
if PATH="${scratch}/bin:${PATH}" REAL_PYTHON3="${real_python}" REAL_GIT="${real_git}" HARNESS_LOG="${log}" FAKE_SNAPSHOT_ROOT="${scratch}/archive" \
  CONSOLE_BUCK_PREFLIGHT_BUCK="${scratch}/buck" \
  CONSOLE_BUCK_PREFLIGHT_ISOLATION_DIR="preflight-lock" FAKE_GENERATED_FACE_GATE_SIGNAL_PARENT=1 "${harness}"; then
  echo "expected an interrupted generated-face gate to fail preflight" >&2
  exit 1
fi
test ! -e "${snapshot}"
test ! -e "${snapshot}.tar"

grep -Fq '2026-07-15' "${playbook}"
grep -Fq 'Cells are trust, toolchain, and configuration boundaries' "${playbook}"
grep -Fq 'No per-module cells' "${playbook}"
grep -Fq 'validate_generated_faces.py' "${harness}"
grep -Fq 'generated_face_registry.json' "${repo_root}/tools/buck/test_validate_generated_faces.py"
grep -Fq 'console-buck2-scale-playbook.md' "${roadmap}"
grep -Fq 'console-buck2-scale-playbook.md' "${ledger}"

echo "preflight: PASS"
