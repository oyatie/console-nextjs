#!/usr/bin/env bash
# Load a strictly formatted, harness-owned PostgreSQL environment file and run
# the Buck-built integration-test binary. Values are treated as data, never code.
set -euo pipefail

env_file="${CONSOLE_BUCK_POSTGRES_ENV_FILE:?Buck PostgreSQL environment file is required}"
if [[ ! -f "${env_file}" ]]; then
  echo "buck-postgres: environment file is not a regular file" >&2
  exit 1
fi

# GNU form first, BSD second. Order is load-bearing: on Linux `stat -f` means
# --file-system, so it SUCCEEDS with a filesystem dump instead of failing, the
# `||` fallback never fires, and the comparison below can never match "600".
# On macOS the GNU `-c` form exits 1, so the BSD fallback is reached correctly.
file_mode="$(stat -c '%a' "${env_file}" 2>/dev/null || stat -f '%Lp' "${env_file}")"
if [[ "${file_mode}" != "600" ]]; then
  echo "buck-postgres: environment file must be mode 0600" >&2
  exit 1
fi

safe_value_pattern='^[A-Za-z0-9:/?%._@+&=-]+$'
have_database_url=0
have_owner_url=0
have_runtime_url=0
have_admin_url=0
while IFS= read -r line || [[ -n "${line}" ]]; do
  case "${line}" in
    *=*) key="${line%%=*}"; value="${line#*=}" ;;
    *) echo "buck-postgres: malformed environment file" >&2; exit 1 ;;
  esac
  [[ "${value}" =~ ${safe_value_pattern} ]] || { echo "buck-postgres: malformed environment file" >&2; exit 1; }
  case "${key}" in
    DATABASE_URL)
      [[ "${have_database_url}" == 0 ]] || { echo "buck-postgres: duplicate environment key" >&2; exit 1; }
      have_database_url=1 ;;
    CONSOLE_APALIS_OWNER_DATABASE_URL)
      [[ "${have_owner_url}" == 0 ]] || { echo "buck-postgres: duplicate environment key" >&2; exit 1; }
      have_owner_url=1 ;;
    CONSOLE_APALIS_RUNTIME_DATABASE_URL)
      [[ "${have_runtime_url}" == 0 ]] || { echo "buck-postgres: duplicate environment key" >&2; exit 1; }
      have_runtime_url=1 ;;
    CONSOLE_APALIS_ADMIN_DATABASE_URL)
      [[ "${have_admin_url}" == 0 ]] || { echo "buck-postgres: duplicate environment key" >&2; exit 1; }
      have_admin_url=1 ;;
    *) echo "buck-postgres: unexpected environment key" >&2; exit 1 ;;
  esac
  export "${key}=${value}"
done <"${env_file}"

if [[ "${have_database_url}${have_owner_url}${have_runtime_url}${have_admin_url}" != "1111" ]]; then
  echo "buck-postgres: incomplete environment file" >&2
  exit 1
fi

exact_test="${CONSOLE_BUCK_RUST_TEST_EXACT:-}"
if [[ -n "${exact_test}" ]]; then
  [[ "${exact_test}" =~ ^[[:alnum:]_:]+$ ]] || {
    echo "buck-postgres: exact Rust test name contains unsupported characters" >&2
    exit 1
  }
  exec "$@" --exact "${exact_test}"
fi
exec "$@"
