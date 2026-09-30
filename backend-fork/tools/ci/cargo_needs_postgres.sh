#!/usr/bin/env bash
# Run candidate-bound disposable-PostgreSQL integration tests via Cargo.
# Same Docker contract as tools/buck/test_needs_postgres.sh; Cargo driver for rust-cache.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
map_path="${repo_root}/tools/ci/postgres-cargo-map.json"
only_csv=""
workflow_only=0
num_threads=1
# cargo | nextest. See the nextest branch below for why this is opt-in per shard.
runner=cargo
shard_id=""
keep_going=1

usage() {
  echo "usage: cargo_needs_postgres.sh [--map PATH] [--workflow-only] [--only name[,name...]] [--shard-id app|platform|ontology|domain-a|domain-b] [--runner cargo|nextest] [--num-threads N] [--keep-going|--fail-fast]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --map) map_path="$2"; shift 2 ;;
    --map=*) map_path="${1#*=}"; shift ;;
    --workflow-only) workflow_only=1; shift ;;
    --only) only_csv="$2"; shift 2 ;;
    --only=*) only_csv="${1#*=}"; shift ;;
    --shard-id) shard_id="$2"; shift 2 ;;
    --shard-id=*) shard_id="${1#*=}"; shift ;;
    --runner) runner="$2"; shift 2 ;;
    --runner=*) runner="${1#*=}"; shift ;;
    --num-threads) num_threads="$2"; shift 2 ;;
    --num-threads=*) num_threads="${1#*=}"; shift ;;
    --keep-going) keep_going=1; shift ;;
    --fail-fast) keep_going=0; shift ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

case "${shard_id}" in
  ""|app|platform|ontology|domain-a|domain-b) ;;
  domain)
    echo "cargo-postgres: --shard-id domain retired in S2; use domain-a or domain-b" >&2
    exit 2
    ;;
  *)
    echo "cargo-postgres: invalid --shard-id ${shard_id} (want app|platform|ontology|domain-a|domain-b)" >&2
    exit 2
    ;;
esac

case "${runner}" in
  cargo|nextest) ;;
  *) echo "cargo-postgres: invalid --runner ${runner} (want cargo|nextest)" >&2; exit 2 ;;
esac

[[ -f "${map_path}" ]] || { echo "cargo-postgres: map missing: ${map_path}" >&2; exit 1; }

postgres_image="postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56"
container_name="console-cargo-postgres-${USER:-user}-$$"
database="console_cargo_test_$$_contract"
container_env_file=""
test_env_file=""

cleanup() {
  docker rm -f "${container_name}" >/dev/null 2>&1 || true
  [[ -z "${container_env_file}" ]] || rm -f "${container_env_file}"
  [[ -z "${test_env_file}" ]] || rm -f "${test_env_file}"
}
trap cleanup EXIT

secret() { openssl rand -hex 32; }
admin_password="$(secret)"
app_password="$(secret)"
runtime_password="$(secret)"
leave_command_password="$(secret)"
ontology_command_password="$(secret)"
platform_force_command_password="$(secret)"

umask 077
container_env_file="$(mktemp "${TMPDIR:-/tmp}/console-cargo-postgres-container.XXXXXX")"
chmod 600 "${container_env_file}"
{
  printf 'POSTGRES_DB=%s\nPOSTGRES_USER=console_buck_admin\nPOSTGRES_PASSWORD=%s\n' "${database}" "${admin_password}"
  printf 'POSTGRES_HOST=127.0.0.1\nPOSTGRES_PORT=5432\nPOSTGRES_ADMIN_USER=console_buck_admin\nPOSTGRES_ADMIN_PASSWORD=%s\n' "${admin_password}"
  printf 'CONSOLE_APP_POSTGRES_PASSWORD=%s\nCONSOLE_RT_POSTGRES_PASSWORD=%s\n' "${app_password}" "${runtime_password}"
  printf 'CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD=%s\n' \
    "${leave_command_password}" "${ontology_command_password}" "${platform_force_command_password}"
} >"${container_env_file}"

if ! docker image inspect "${postgres_image}" >/dev/null 2>&1; then
  pull_attempts=0
  pull_backoff="3 9 27"
  for backoff in ${pull_backoff}; do
    if docker pull --quiet "${postgres_image}" >/dev/null 2>&1; then
      break
    fi
    pull_attempts=$((pull_attempts + 1))
    if [[ "${pull_attempts}" -ge 3 ]]; then
      docker pull "${postgres_image}" >&2 || true
      echo "cargo-postgres: could not pull pinned PostgreSQL image" >&2
      exit 1
    fi
    sleep "${backoff}"
  done
fi

docker run -d --rm --name "${container_name}" -p 127.0.0.1::5432 \
  --env-file "${container_env_file}" "${postgres_image}" \
  -c fsync=off -c synchronous_commit=off -c full_page_writes=off >/dev/null
docker cp "${repo_root}/ops/postgres-reconcile-topology.sh" "${container_name}:/topology.sh"
docker cp "${container_env_file}" "${container_name}:/topology.env"

for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
  if ! pid1_comm="$(docker exec "${container_name}" cat /proc/1/comm 2>/dev/null)"; then
    if [[ "${attempt}" == 30 ]]; then echo "cargo-postgres: cannot inspect PID 1" >&2; exit 1; fi
    sleep 1; continue
  fi
  if [[ "${pid1_comm}" == "postgres" ]] && docker exec "${container_name}" pg_isready -h 127.0.0.1 -U console_buck_admin -d "${database}" >/dev/null 2>&1; then break; fi
  if [[ "${attempt}" == 30 ]]; then echo "cargo-postgres: not healthy" >&2; exit 1; fi
  sleep 1
done
docker exec "${container_name}" sh -ceu 'set -a; . /topology.env; exec bash /topology.sh'

# The reconcile above runs BEFORE any migration, so its canonical
# writer-ownership census sees zero canonical tables and enforces nothing. Run
# it again on a disposable probe database that has the real schema, with the
# zero-table guard armed. Nothing here touches "${database}", so the cargo tests
# below still see exactly the database they saw before.
bash "${repo_root}/backend/ci/gates/writer-ownership/canonical-enforce.sh" \
  "${repo_root}" "${container_name}" "canonical_probe_$$"

port_mapping="$(docker port "${container_name}" 5432/tcp)"
port="${port_mapping##*:}"
case "${port}" in
  ''|*[!0-9]*) echo "cargo-postgres: bad port" >&2; exit 1 ;;
esac
database_url="postgres://console_buck_admin:${admin_password}@127.0.0.1:${port}/${database}?options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1"
apalis_owner_database_url="postgres://console_app:${app_password}@127.0.0.1:${port}/${database}"
apalis_runtime_database_url="postgres://console_rt:${runtime_password}@127.0.0.1:${port}/${database}"
test_env_file="$(mktemp "${TMPDIR:-/tmp}/console-cargo-postgres-env.XXXXXX")"
chmod 600 "${test_env_file}"
{
  printf 'DATABASE_URL=%s\n' "${database_url}"
  printf 'CONSOLE_APALIS_OWNER_DATABASE_URL=%s\n' "${apalis_owner_database_url}"
  printf 'CONSOLE_APALIS_RUNTIME_DATABASE_URL=%s\n' "${apalis_runtime_database_url}"
  printf 'CONSOLE_APALIS_ADMIN_DATABASE_URL=%s\n' "${database_url}"
} >"${test_env_file}"

while IFS= read -r line || [[ -n "${line}" ]]; do
  case "${line}" in
    *=*) key="${line%%=*}"; value="${line#*=}"; export "${key}=${value}" ;;
  esac
done <"${test_env_file}"

export SQLX_OFFLINE=true
export RUST_TEST_THREADS="${num_threads}"
export CARGO_TERM_COLOR=always

tmp_list="$(mktemp "${TMPDIR:-/tmp}/console-cargo-list.XXXXXX")"
# Shard selection has ONE implementation: tools/ci/postgres-partition.mjs.
# This used to be a Python copy of the family logic in postgres-shard.mjs,
# kept in step with the JS by hand -- two implementations of a rule that
# decides which tests run is a false-green waiting to happen.
node "${repo_root}/tools/ci/postgres-partition.mjs" \
  --emit-shard="${shard_id}" --only="${only_csv}" "${map_path}" >"${tmp_list}" || {
  echo "cargo-postgres: shard partition failed; refusing to run a partial set" >&2
  rm -f "${tmp_list}"
  exit 1
}

if [[ ! -s "${tmp_list}" ]]; then
  echo "cargo-postgres: no map entries selected" >&2
  exit 1
fi

count="$(wc -l <"${tmp_list}" | tr -d ' ')"
if [[ -n "${shard_id}" ]]; then
  echo "cargo-postgres: running ${count} cargo test invocations (shard=${shard_id} threads=${num_threads})"
else
  echo "cargo-postgres: running ${count} cargo test invocations (threads=${num_threads})"
fi

# Unique packages for --no-run build
tmp_pkgs="$(mktemp "${TMPDIR:-/tmp}/console-cargo-pkgs.XXXXXX")"
python3 - "${tmp_list}" >"${tmp_pkgs}" <<'PY'
import json, sys
pkgs=set()
for line in open(sys.argv[1]):
    pkgs.add(json.loads(line)["package"])
for p in sorted(pkgs):
    print(p)
PY

build_args=(cargo test --locked --manifest-path "${repo_root}/backend/Cargo.toml" --no-run)
while IFS= read -r p; do
  [[ -n "${p}" ]] && build_args+=(-p "${p}")
done <"${tmp_pkgs}"
echo "cargo-postgres: building packages..."
( cd "${repo_root}" && "${build_args[@]}" )

# Fail-slow sweep: run every selected binary, collect per-binary pass/fail, print
# a summary table, and exit non-zero if any failed. The keep-going loop lives in
# cargo-test-runner.sh so it is unit-testable without Docker (fake map + stubbed
# cargo). Default is --keep-going; --fail-fast opts back out for local use.
if [[ "${runner}" == nextest ]]; then
  # One `cargo nextest run` over a filterset instead of N serial `cargo test`
  # invocations, each pinned to --test-threads=1.
  #
  # Measured 2026-08-18 on run 32115833327: 209 invocations, 3299.4s of test
  # execution, 88% of it in #[sqlx::test] cases that are serial only because the
  # cargo path forces them to be. `.config/nextest.toml` already encodes the
  # parallel/serial split this repo decided on (ADR-0039 / DN-0005 P3): only the
  # `cluster-global` group is max-threads=1 and its comment says the rest stay
  # parallel. Nothing has ever invoked it -- the ledger records the runner swap
  # as deferred on "preflight command locks", not on a safety concern.
  #
  # The filterset is derived from the SAME map rows the cargo path would run, so
  # the two runners select an identical target set by construction. The
  # translator exits non-zero on any row it cannot map rather than silently
  # narrowing the selection.
  filterset="$(node "${repo_root}/tools/ci/nextest-filterset.mjs" <"${tmp_list}")" || {
    echo "cargo-postgres: filterset translation failed; refusing to run a narrowed set" >&2
    rm -f "${tmp_list}" "${tmp_pkgs}"
    exit 1
  }
  # --config-file is REQUIRED, not decoration. nextest resolves its config from
  # <workspace-root>/.config/nextest.toml, and this workspace root is backend/,
  # while .config/nextest.toml lives at the REPO root. Measured 2026-08-18:
  # without this flag nextest reports `profile 'ci' not found (known profiles:
  # default, default-miri)` -- it had read no config at all, which silently
  # disables the `cluster-global` max-threads=1 group that ADR-0039 / DN-0005 P3
  # landed as the serial safety mechanism. The config existing is not the same
  # as the tool reading it.
  nextest_args=(cargo nextest run --locked --manifest-path "${repo_root}/backend/Cargo.toml"
    --config-file "${repo_root}/.config/nextest.toml" --profile ci)
  while IFS= read -r p; do
    [[ -n "${p}" ]] && nextest_args+=(-p "${p}")
  done <"${tmp_pkgs}"
  nextest_args+=(-E "${filterset}")
  echo "cargo-postgres: running ${count} targets via cargo-nextest (shard=${shard_id})"
  ( cd "${repo_root}" && SQLX_OFFLINE=true CARGO_TERM_COLOR=always "${nextest_args[@]}" )
  status=$?
  rm -f "${tmp_list}" "${tmp_pkgs}"
  exit "${status}"
fi

export CARGO_REPO_ROOT="${repo_root}"
# Attributes each `cargo-postgres-timing:` line to its shard, so durations
# harvested from five separate job logs can be re-packed as one population.
export CARGO_POSTGRES_SHARD_ID="${shard_id}"
export RUST_TEST_THREADS="${num_threads}"
if [[ "${keep_going}" == 1 ]]; then
  "${repo_root}/tools/ci/cargo-test-runner.sh" --keep-going <"${tmp_list}"
else
  "${repo_root}/tools/ci/cargo-test-runner.sh" --fail-fast <"${tmp_list}"
fi
status=$?
rm -f "${tmp_list}" "${tmp_pkgs}"
exit "${status}"
