#!/usr/bin/env bash
# Run Buck2 tests that need PostgreSQL against a disposable local container.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
postgres_image="postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56"
container_name="console-buck-postgres-${USER:-user}-$$"
buck_bin="${CONSOLE_BUCK_NEEDS_POSTGRES_TEST_BUCK:-${repo_root}/tools/buck2}"
isolation_name="${CONSOLE_BUCK_NEEDS_POSTGRES_ISOLATION_DIR:-}"
if [[ -n "${isolation_name}" && ! "${isolation_name}" =~ ^[[:alnum:]_.-]+$ ]]; then
  echo "buck-postgres: isolation name must contain only letters, digits, dot, underscore, or dash" >&2
  exit 1
fi
database="console_buck_test_$$_contract"
container_env_file=""
test_env_file=""
active_buck_pid=""
exact_test="${CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT:-}"
if [[ -n "${exact_test}" && ! "${exact_test}" =~ ^[[:alnum:]_:]+$ ]]; then
  echo "buck-postgres: exact Rust test name contains unsupported characters" >&2
  exit 1
fi
for arg in "$@"; do
  case "${arg}" in
    //backend/*|root//backend/*)
      echo "buck-postgres: raw backend test targets bypass the credential loader; use a //tools/buck PostgreSQL wrapper" >&2
      exit 2
      ;;
  esac
done

cleanup() {
  docker rm -f "${container_name}" >/dev/null 2>&1 || true
  [[ -z "${container_env_file}" ]] || rm -f "${container_env_file}"
  [[ -z "${test_env_file}" ]] || rm -f "${test_env_file}"
}

on_signal() {
  local status="$1"
  trap - HUP INT TERM
  if [[ -n "${active_buck_pid}" ]]; then
    kill -TERM "${active_buck_pid}" >/dev/null 2>&1 || true
    wait "${active_buck_pid}" >/dev/null 2>&1 || true
    active_buck_pid=""
  fi
  exit "${status}"
}
trap cleanup EXIT
trap 'on_signal 129' HUP
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

secret() { openssl rand -hex 32; }
admin_password="$(secret)"
app_password="$(secret)"
runtime_password="$(secret)"
leave_command_password="$(secret)"
ontology_command_password="$(secret)"
platform_force_command_password="$(secret)"
passwords=("${admin_password}" "${app_password}" "${runtime_password}" "${leave_command_password}" "${ontology_command_password}" "${platform_force_command_password}")
for ((i = 0; i < ${#passwords[@]}; i++)); do
  for ((j = i + 1; j < ${#passwords[@]}; j++)); do
    if [[ "${passwords[i]}" == "${passwords[j]}" ]]; then
      echo "buck-postgres: generated passwords must be pairwise distinct" >&2
      exit 1
    fi
  done
done

umask 077
container_env_file="$(mktemp "${TMPDIR:-/tmp}/console-buck-postgres-container.XXXXXX")"
chmod 600 "${container_env_file}"
{
  printf 'POSTGRES_DB=%s\nPOSTGRES_USER=console_buck_admin\nPOSTGRES_PASSWORD=%s\n' "${database}" "${admin_password}"
  printf 'POSTGRES_HOST=127.0.0.1\nPOSTGRES_PORT=5432\nPOSTGRES_ADMIN_USER=console_buck_admin\nPOSTGRES_ADMIN_PASSWORD=%s\n' "${admin_password}"
  printf 'CONSOLE_APP_POSTGRES_PASSWORD=%s\nCONSOLE_RT_POSTGRES_PASSWORD=%s\n' "${app_password}" "${runtime_password}"
  printf 'CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD=%s\n' "${leave_command_password}" "${ontology_command_password}" "${platform_force_command_password}"
} >"${container_env_file}"

# Pull EXPLICITLY, with bounded retry, before running.
#
# `docker run` pulls implicitly, so an unreachable registry surfaced as a bare
# `exit 125` from the run itself — no retry, and nothing distinguishing a registry
# outage from a broken harness. That reddened CI on 2026-07-31:
#
#   Unable to find image 'postgres:18.4@sha256:65f70a15...' locally
#   docker: Error response from daemon: Get "https://registry-1.docker.io/v2/": context deadline exceeded
#
# The image is digest-pinned, so retrying cannot fetch different content: either the
# digest resolves or the pull fails. This makes an already-exact fetch survive a
# transient registry and NOTHING else — no fallback tag, no alternate registry, no
# unpinned image. A registry that is genuinely down still fails the run.
if ! docker image inspect "${postgres_image}" >/dev/null 2>&1; then
  pull_attempts=0
  pull_backoff=(3 9 27)
  until docker pull --quiet "${postgres_image}" >/dev/null 2>&1; do
    if (( pull_attempts >= ${#pull_backoff[@]} )); then
      # Surface the registry's own error instead of a bare 125 from `run`.
      docker pull "${postgres_image}" >&2 || true
      echo "buck-postgres: could not pull the pinned PostgreSQL image after $((${#pull_backoff[@]} + 1)) attempts" >&2
      exit 1
    fi
    echo "buck-postgres: pulling the pinned PostgreSQL image failed; retrying" >&2
    sleep "${pull_backoff[pull_attempts]}"
    pull_attempts=$((pull_attempts + 1))
  done
fi

docker run -d --rm --name "${container_name}" -p 127.0.0.1::5432 \
  --env-file "${container_env_file}" "${postgres_image}" \
  -c fsync=off -c synchronous_commit=off -c full_page_writes=off >/dev/null
docker cp "${repo_root}/ops/postgres-reconcile-topology.sh" "${container_name}:/topology.sh"
docker cp "${container_env_file}" "${container_name}:/topology.env"

for attempt in {1..30}; do
  if ! pid1_comm="$(docker exec "${container_name}" cat /proc/1/comm 2>/dev/null)"; then
    if [[ "${attempt}" == 30 ]]; then
      echo "buck-postgres: could not inspect disposable PostgreSQL PID 1 after 30 attempts" >&2
      exit 1
    fi
    sleep 1
    continue
  fi
  if [[ "${pid1_comm}" == "postgres" ]] && docker exec "${container_name}" pg_isready -h 127.0.0.1 -U console_buck_admin -d "${database}" >/dev/null 2>&1; then break; fi
  if [[ "${attempt}" == 30 ]]; then echo "buck-postgres: disposable PostgreSQL did not become healthy" >&2; exit 1; fi
  sleep 1
done
# The protected file path, not any value, is in Docker's host argv.
docker exec "${container_name}" sh -ceu 'set -a; . /topology.env; exec bash /topology.sh'

port_mapping="$(docker port "${container_name}" 5432/tcp)"
port="${port_mapping##*:}"
if [[ ! "${port}" =~ ^[0-9]+$ ]]; then echo "buck-postgres: could not resolve disposable PostgreSQL loopback port" >&2; exit 1; fi
database_url="postgres://console_buck_admin:${admin_password}@127.0.0.1:${port}/${database}?options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1"
apalis_owner_database_url="postgres://console_app:${app_password}@127.0.0.1:${port}/${database}"
apalis_runtime_database_url="postgres://console_rt:${runtime_password}@127.0.0.1:${port}/${database}"
test_env_file="$(mktemp "${TMPDIR:-/tmp}/console-buck-postgres-env.XXXXXX")"
chmod 600 "${test_env_file}"
{
  printf 'DATABASE_URL=%s\n' "${database_url}"
  printf 'CONSOLE_APALIS_OWNER_DATABASE_URL=%s\n' "${apalis_owner_database_url}"
  printf 'CONSOLE_APALIS_RUNTIME_DATABASE_URL=%s\n' "${apalis_runtime_database_url}"
  printf 'CONSOLE_APALIS_ADMIN_DATABASE_URL=%s\n' "${database_url}"
} >"${test_env_file}"

# Only a mode-0600 path crosses Buck's test-executor argv. The wrapper parses
# this fixed data file inside the test process without evaluating its contents.
test_executor_args=(--env "CONSOLE_BUCK_POSTGRES_ENV_FILE=${test_env_file}" --env RUST_TEST_THREADS=1)
if [[ -n "${exact_test}" ]]; then
  test_executor_args+=(--env "CONSOLE_BUCK_RUST_TEST_EXACT=${exact_test}")
fi
# BUILD IN PARALLEL, THEN TEST SERIALLY.
#
# Buck2's `-j/--num-threads` is "Number of threads to use during execution (default is # cores)"
# — it governs the WHOLE invocation, compile actions included. CI passes `--num-threads=1` to keep
# the test phase serialized, and that flag was therefore also compiling every action
# single-threaded on a multi-core runner.
#
# Serialization is only needed for the TEST phase, and it is needed for a specific reason: three
# test files issue cluster-global `ALTER ROLE ... PASSWORD`, which outlives the per-test database
# `#[sqlx::test]` drops and poisons siblings sharing this one container. Compiling has no such
# constraint.
#
# So the build is hoisted into its own pass at default parallelism. Targets are recognised by
# their `//` prefix, which is how the caller passes them; every other argv entry is a buck flag
# and is deliberately not forwarded here — `--num-threads=1` least of all. The daemon carries the
# result into the test pass below, so nothing is rebuilt.
build_targets=()
for arg in "$@"; do
  case "${arg}" in
    //*|root//*) build_targets+=("${arg}") ;;
  esac
done
if ((${#build_targets[@]} > 0)); then
  if [[ -n "${isolation_name}" ]]; then
    BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" build --local-only "${build_targets[@]}"
  else
    "${buck_bin}" build --local-only "${build_targets[@]}"
  fi
fi

# Reuse the caller's/default Buck daemon so PostgreSQL integration tests share
# the same-worktree analysis and compile cache. Callers that need an isolated
# daemon can still opt in explicitly without forcing every run cold.
if [[ -n "${isolation_name}" ]]; then
  BUCK_ISOLATION_DIR="${isolation_name}" "${buck_bin}" test --local-only "$@" \
    -- "${test_executor_args[@]}" &
else
  "${buck_bin}" test --local-only "$@" -- "${test_executor_args[@]}" &
fi
active_buck_pid="$!"
if wait "${active_buck_pid}"; then
  buck_status=0
else
  buck_status="$?"
fi
active_buck_pid=""
exit "${buck_status}"
