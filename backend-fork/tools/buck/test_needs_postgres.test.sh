#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
harness="${repo_root}/tools/buck/test_needs_postgres.sh"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/console-buck-postgres-test.XXXXXX")"
fake_bin="${scratch}/bin"; log="${scratch}/calls.log"; mkdir -p "${fake_bin}"
trap 'rm -rf "${scratch}"' EXIT
cat >"${fake_bin}/docker" <<'DOCKER'
#!/usr/bin/env bash
{ printf 'docker'; printf ' %q' "$@"; printf '\n'; } >>"${HARNESS_LOG}"
sequence_value() {
  local sequence="$1" attempt_file="$2" default="$3"
  if [[ -z "${sequence}" ]]; then printf '%s' "${default}"; return; fi
  local attempt=0 index
  [[ -f "${attempt_file}" ]] && attempt="$(cat "${attempt_file}")"
  attempt=$((attempt + 1)); printf '%s' "${attempt}" >"${attempt_file}"
  local -a values
  IFS=, read -r -a values <<<"${sequence}"
  index=$((attempt - 1)); (( index < ${#values[@]} )) || index=$((${#values[@]} - 1))
  printf '%s' "${values[index]}"
}
case "$1" in
  run) echo fake-container ;;
  cp)
    if [[ "$3" == *:/topology.env ]]; then
      cut -d= -f1 "$2" | sort >"${HARNESS_LOG}.topology-env-keys"
      printf '%s\n' "$2" >"${HARNESS_LOG}.topology-env-file"
    fi
    exit 0 ;;
  exec)
    if [[ " $* " == *" /proc/1/comm "* ]]; then
      pid1_status="$(sequence_value "${FAKE_DOCKER_PID1_STATUS_SEQUENCE:-}" "${HARNESS_LOG}.pid1-status-attempt" "${FAKE_DOCKER_PID1_STATUS:-0}")"
      if [[ "${pid1_status}" != 0 ]]; then
        printf '%s\n' "${FAKE_DOCKER_PID1_ERROR_OUTPUT:-fake PID1 probe failed}" >&2
        exit "${pid1_status}"
      fi
      sequence_value "${FAKE_DOCKER_PID1_COMM_SEQUENCE:-}" "${HARNESS_LOG}.pid1-attempt" "${FAKE_DOCKER_PID1_COMM:-postgres}"
      printf '\n'
      exit 0
    fi
    if [[ " $* " == *" pg_isready "* ]]; then
      readiness_status="$(sequence_value "${FAKE_DOCKER_READINESS_STATUS_SEQUENCE:-}" "${HARNESS_LOG}.readiness-attempt" "${FAKE_DOCKER_READINESS_STATUS:-0}")"
      exit "${readiness_status}"
    fi
    if [[ " $* " == *" /topology.sh "* ]]; then
      printf 'topology\n' >>"${HARNESS_LOG}"
      exit "${FAKE_DOCKER_EXEC_STATUS:-0}"
    fi
    exit 0 ;;
  image) exit "${FAKE_DOCKER_IMAGE_INSPECT_STATUS:-1}" ;;
  pull) exit "$(sequence_value "${FAKE_DOCKER_PULL_STATUS_SEQUENCE:-}" "${HARNESS_LOG}.pull-attempt" "${FAKE_DOCKER_PULL_STATUS:-0}")" ;;
  port) echo 127.0.0.1:49123 ;;
  rm) exit 0 ;;
  *) exit 1 ;;
esac
DOCKER
cat >"${fake_bin}/openssl" <<'OPENSSL'
#!/usr/bin/env bash
count_file="${HARNESS_LOG}.secrets"; count=0; [[ -f "${count_file}" ]] && count="$(cat "${count_file}")"; count=$((count+1)); printf '%s' "${count}" >"${count_file}"; printf 'secret-%s\n' "${count}"
OPENSSL
cat >"${scratch}/buck" <<'BUCK'
#!/usr/bin/env bash
{ printf 'buck'; printf ' %q' "$@"; printf '\n'; } >>"${HARNESS_LOG}"
printf 'buck-isolation %s\n' "${BUCK_ISOLATION_DIR-<unset>}" >>"${HARNESS_LOG}"
env_file=""; for arg in "$@"; do case "${arg}" in CONSOLE_BUCK_POSTGRES_ENV_FILE=*) env_file="${arg#*=}";; esac; done
[[ -f "${env_file}" && "$(stat -f '%Lp' "${env_file}")" == 600 ]]
grep -Fq 'DATABASE_URL=postgres://console_buck_admin:' "${env_file}"
grep -Fq 'CONSOLE_APALIS_OWNER_DATABASE_URL=postgres://console_app:' "${env_file}"
grep -Fq 'CONSOLE_APALIS_RUNTIME_DATABASE_URL=postgres://console_rt:' "${env_file}"
grep -Fq 'CONSOLE_APALIS_ADMIN_DATABASE_URL=postgres://console_buck_admin:' "${env_file}"
printf '%s\n' "${env_file}" >>"${HARNESS_LOG}.envfiles"
if [[ "${FAKE_BUCK_SLEEP:-0}" == 1 ]]; then printf "%s\n" "$$" >"${HARNESS_LOG}.childpid"; exec sleep 30; fi
exit "${FAKE_BUCK_STATUS:-0}"
BUCK
cat >"${fake_bin}/sleep" <<'SLEEP'
#!/usr/bin/env bash
if [[ "${FAKE_SLEEP_INSTANT:-0}" == 1 ]]; then exit 0; fi
exec /bin/sleep "$@"
SLEEP
chmod +x "${fake_bin}/docker" "${fake_bin}/openssl" "${fake_bin}/sleep" "${scratch}/buck"
raw_target_log="${scratch}/raw-target.log"
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${raw_target_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" "${harness}" //backend/app:console-app-itest-org_change_api; then exit 1; fi
! grep -q '^docker' "${raw_target_log}" 2>/dev/null
! grep -q '^buck' "${raw_target_log}" 2>/dev/null
entrypoint_ready_log="${scratch}/entrypoint-ready.log"
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${entrypoint_ready_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_DOCKER_PID1_COMM_SEQUENCE=docker-entrypoint.sh FAKE_SLEEP_INSTANT=1 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
[[ "$(grep -c 'cat /proc/1/comm' "${entrypoint_ready_log}")" == 30 ]]
[[ "$(grep -c 'pg_isready ' "${entrypoint_ready_log}")" == 0 ]]
if grep -Fxq 'topology' "${entrypoint_ready_log}"; then exit 1; fi
if grep -q '^buck' "${entrypoint_ready_log}" 2>/dev/null; then exit 1; fi
pid1_probe_failure_log="${scratch}/pid1-probe-failure.log"
set +e
pid1_probe_failure_output="$(PATH="${fake_bin}:${PATH}" HARNESS_LOG="${pid1_probe_failure_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_DOCKER_PID1_STATUS_SEQUENCE=1 FAKE_DOCKER_PID1_ERROR_OUTPUT='secret-probe postgres://console_buck_admin:secret-password@127.0.0.1:5432/console_buck_test' FAKE_SLEEP_INSTANT=1 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres 2>&1)"
pid1_probe_failure_status=$?
set -e
[[ "${pid1_probe_failure_status}" != 0 ]]
grep -Fq 'could not inspect disposable PostgreSQL PID 1 after 30 attempts' <<<"${pid1_probe_failure_output}"
if grep -Fq 'secret-probe' <<<"${pid1_probe_failure_output}"; then exit 1; fi
if grep -Fq 'postgres://' <<<"${pid1_probe_failure_output}"; then exit 1; fi
[[ "$(grep -c 'cat /proc/1/comm' "${pid1_probe_failure_log}")" == 30 ]]
[[ "$(grep -c 'pg_isready ' "${pid1_probe_failure_log}")" == 0 ]]
if grep -Fxq 'topology' "${pid1_probe_failure_log}"; then exit 1; fi
if grep -q '^buck' "${pid1_probe_failure_log}" 2>/dev/null; then exit 1; fi
tcp_not_ready_log="${scratch}/tcp-not-ready.log"
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${tcp_not_ready_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_DOCKER_PID1_COMM_SEQUENCE=postgres FAKE_DOCKER_READINESS_STATUS_SEQUENCE=1 FAKE_SLEEP_INSTANT=1 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
[[ "$(grep -c 'cat /proc/1/comm' "${tcp_not_ready_log}")" == 30 ]]
[[ "$(grep -c 'pg_isready -h 127.0.0.1 -U console_buck_admin -d ' "${tcp_not_ready_log}")" == 30 ]]
if grep -Fxq 'topology' "${tcp_not_ready_log}"; then exit 1; fi
if grep -q '^buck' "${tcp_not_ready_log}" 2>/dev/null; then exit 1; fi
recovery_log="${scratch}/recovery.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${recovery_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_DOCKER_PID1_COMM_SEQUENCE=docker-entrypoint.sh,postgres,postgres FAKE_DOCKER_READINESS_STATUS_SEQUENCE=1,0 FAKE_SLEEP_INSTANT=1 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
[[ "$(grep -c 'cat /proc/1/comm' "${recovery_log}")" == 3 ]]
[[ "$(grep -c 'pg_isready -h 127.0.0.1 -U console_buck_admin -d ' "${recovery_log}")" == 2 ]]
[[ "$(grep -c '^topology$' "${recovery_log}")" == 1 ]]
[[ "$(grep -c '^buck test ' "${recovery_log}")" == 1 ]]
topology_line="$(grep -n '^topology$' "${recovery_log}" | cut -d: -f1)"
readiness_line="$(grep -n 'pg_isready -h 127.0.0.1 -U console_buck_admin -d ' "${recovery_log}" | tail -1 | cut -d: -f1)"
buck_line="$(grep -n '^buck test ' "${recovery_log}" | cut -d: -f1)"
[[ "${topology_line}" -gt "${readiness_line}" ]]
[[ "${buck_line}" -gt "${topology_line}" ]]
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
calls="$(cat "${log}")"; buck_calls="$(grep '^buck' "${log}")"
grep -Fxq 'buck-isolation <unset>' "${log}"
grep -Fq -- '--env-file ' <<<"${calls}"
grep -Fq -- ':/topology.env' <<<"${calls}"
grep -Fq -- 'sh -ceu set\ -a\;\ .\ /topology.env\;\ exec\ bash\ /topology.sh' <<<"${calls}"
grep -Fq -- 'CONSOLE_BUCK_POSTGRES_ENV_FILE=' <<<"${buck_calls}"
! grep -Fq -- 'secret-' <<<"${calls}"
! grep -Fq -- 'postgres://' <<<"${calls}"
expected_topology_env_keys='CONSOLE_APP_POSTGRES_PASSWORD
CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD
CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD
CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD
CONSOLE_RT_POSTGRES_PASSWORD
POSTGRES_ADMIN_PASSWORD
POSTGRES_ADMIN_USER
POSTGRES_DB
POSTGRES_HOST
POSTGRES_PASSWORD
POSTGRES_PORT
POSTGRES_USER'
[[ "$(cat "${log}.topology-env-keys")" == "${expected_topology_env_keys}" ]]
! grep -Fq 'CONSOLE_PLATFORM_FORCE_COMMAND_PASSWORD' "${log}.topology-env-keys"
while IFS= read -r envfile; do [[ ! -e "${envfile}" ]]; done <"${log}.envfiles"
exact_log="${scratch}/exact.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${exact_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT=one_exact_test "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
grep -Fq 'CONSOLE_BUCK_RUST_TEST_EXACT=one_exact_test' "${exact_log}"
! grep -Fq -- 'secret-' "${exact_log}"
isolation_log="${scratch}/isolation.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${isolation_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" CONSOLE_BUCK_NEEDS_POSTGRES_ISOLATION_DIR=postgres-proof "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
grep -Fxq 'buck-isolation postgres-proof' "${isolation_log}"
inherited_isolation_log="${scratch}/inherited-isolation.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${inherited_isolation_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" BUCK_ISOLATION_DIR=caller-proof "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
grep -Fxq 'buck-isolation caller-proof' "${inherited_isolation_log}"
invalid_isolation_log="${scratch}/invalid-isolation.log"
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${invalid_isolation_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" CONSOLE_BUCK_NEEDS_POSTGRES_ISOLATION_DIR='bad isolation' "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
! grep -q '^buck' "${invalid_isolation_log}" 2>/dev/null
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${exact_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT='bad test' "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
setup_failure_log="${scratch}/setup-failure.log"
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${setup_failure_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_DOCKER_EXEC_STATUS=23 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
grep -Fq 'docker rm -f' "${setup_failure_log}"
! grep -q '^buck' "${setup_failure_log}"
! grep -Fq -- 'secret-' "${setup_failure_log}"
[[ ! -e "$(cat "${setup_failure_log}.topology-env-file")" ]]
if PATH="${fake_bin}:${PATH}" HARNESS_LOG="${log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_BUCK_STATUS=17 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres; then exit 1; fi
while IFS= read -r envfile; do [[ ! -e "${envfile}" ]]; done <"${log}.envfiles"
signal_log="${scratch}/signal.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${signal_log}" CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" FAKE_BUCK_SLEEP=1 "${harness}" //tools/buck:pr473-ontology-key-revision-postgres &
harness_pid=$!
for _ in {1..50}; do [[ -s "${signal_log}.childpid" && -s "${signal_log}.envfiles" ]] && break; sleep 0.1; done
[[ -s "${signal_log}.childpid" && -s "${signal_log}.envfiles" ]]
child_pid="$(cat "${signal_log}.childpid")"
kill -TERM "${harness_pid}"
set +e
wait "${harness_pid}"
signal_status=$?
set -e
[[ "${signal_status}" == 143 ]]
! kill -0 "${child_pid}" 2>/dev/null
while IFS= read -r envfile; do [[ ! -e "${envfile}" ]]; done <"${signal_log}.envfiles"
! grep -Fq -- 'secret-' "${signal_log}"
! grep -Fq -- 'postgres://' "${signal_log}"
# The pinned image is pulled explicitly with bounded retry, because `docker run`'s
# implicit pull turned a registry timeout into a bare `exit 125` with no retry —
# observed reddening CI on 2026-07-31 with "context deadline exceeded".

# A transient registry recovers: two failures then success, and the run proceeds.
pull_recovery_log="${scratch}/pull-recovery.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${pull_recovery_log}" \
  CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" \
  FAKE_DOCKER_PULL_STATUS_SEQUENCE=1,1,0 FAKE_SLEEP_INSTANT=1 \
  "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
[[ "$(grep -c '^docker pull' "${pull_recovery_log}")" == 3 ]]
[[ "$(grep -c '^docker run' "${pull_recovery_log}")" == 1 ]]

# A registry that stays down FAILS the run and never starts a container. This is the
# inversion of the old behaviour, where `docker run` pulled implicitly and died 125.
pull_exhausted_log="${scratch}/pull-exhausted.log"
set +e
pull_exhausted_output="$(PATH="${fake_bin}:${PATH}" HARNESS_LOG="${pull_exhausted_log}" \
  CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" \
  FAKE_DOCKER_PULL_STATUS=1 FAKE_SLEEP_INSTANT=1 \
  "${harness}" //tools/buck:pr473-ontology-key-revision-postgres 2>&1)"
pull_exhausted_status=$?
set -e
[[ "${pull_exhausted_status}" != 0 ]]
grep -Fq 'could not pull the pinned PostgreSQL image after 4 attempts' <<<"${pull_exhausted_output}"
# The load-bearing assertion: no container is ever started.
[[ "$(grep -c '^docker run' "${pull_exhausted_log}")" == 0 ]]
# Retry must not leak a generated password into logs or output.
! grep -Fq -- 'secret-' "${pull_exhausted_log}"
! grep -Fq -- 'postgres://' <<<"${pull_exhausted_output}"

# An image already present locally is NOT re-pulled.
pull_cached_log="${scratch}/pull-cached.log"
PATH="${fake_bin}:${PATH}" HARNESS_LOG="${pull_cached_log}" \
  CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK="${scratch}/buck" \
  FAKE_DOCKER_IMAGE_INSPECT_STATUS=0 FAKE_SLEEP_INSTANT=1 \
  "${harness}" //tools/buck:pr473-ontology-key-revision-postgres
[[ "$(grep -c '^docker pull' "${pull_cached_log}")" == 0 ]]
[[ "$(grep -c '^docker run' "${pull_cached_log}")" == 1 ]]

echo 'test_needs_postgres: PASS'
