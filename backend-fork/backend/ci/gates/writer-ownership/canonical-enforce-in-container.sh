#!/usr/bin/env bash
# Runs inside the PostgreSQL container. See canonical-enforce.sh for the
# contract. The ORDER of the four steps is the whole point: migrations before
# enforcement, and enforcement demanding that there be something to enforce on.
set -euo pipefail

probe_db="${CANONICAL_PROBE_DB:?}"
skip_migrations="${CANONICAL_SKIP_MIGRATIONS:-0}"
extra_sql="${CANONICAL_EXTRA_SQL:-}"
require_tables="${CANONICAL_REQUIRE_TABLES:-1}"

set -a
# shellcheck disable=SC1091
. /topology.env
set +a

admin() {
  PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}" psql \
    --host "${POSTGRES_HOST}" --port "${POSTGRES_PORT}" \
    --username "${POSTGRES_ADMIN_USER}" --set ON_ERROR_STOP=1 --quiet "$@"
}

# 0. A disposable probe database, so nothing here can perturb the harness's own
#    test database -- and a clean slate of ROLES, which a database is not. A role
#    is cluster-wide, so a rogue planted by one probe is still there for the next
#    one; `pg_write_all_data` held by a leftover role fails every run after it,
#    and the probes' own `DROP ROLE IF EXISTS` only covers the role each creates.
admin --dbname "${POSTGRES_DB}" -c "DROP DATABASE IF EXISTS ${probe_db} WITH (FORCE)" >/dev/null
admin --dbname "${POSTGRES_DB}" >/dev/null <<'PSQL'
SELECT format('DROP ROLE %I', rolname) FROM pg_roles WHERE rolname LIKE 'console\_rogue\_%'
\gexec
PSQL
admin --dbname "${POSTGRES_DB}" -c "CREATE DATABASE ${probe_db}" >/dev/null

# 1. Role topology on the probe database. Same script, same ALTER DEFAULT
#    PRIVILEGES, so console_rt ends up holding exactly what it holds in
#    production.
POSTGRES_DB="${probe_db}" bash /topology.sh >/dev/null

# 2. MIGRATIONS. Applied as the owner, in file order -- the zero-padded numeric
#    prefixes make lexicographic order the migration order.
if [[ "${skip_migrations}" != "1" ]]; then
  for file in /canonical-migrations/*.sql; do
    # Match SQLx: ordinary files run atomically; only explicit no-transaction
    # migrations may use statements such as CREATE INDEX CONCURRENTLY.
    psql_transaction=(--single-transaction)
    if [[ "$(head -n 1 "${file}")" == "-- no-transaction"* ]]; then
      psql_transaction=()
    fi
    PGPASSWORD="${CONSOLE_APP_POSTGRES_PASSWORD}" psql \
      --host "${POSTGRES_HOST}" --port "${POSTGRES_PORT}" \
      --username console_app --dbname "${probe_db}" \
      --set ON_ERROR_STOP=1 --quiet "${psql_transaction[@]}" --file "${file}" >/dev/null || {
        echo "canonical-enforce: migration failed: ${file}" >&2
        exit 1
      }
  done
fi

# 3. Optional planted grant, after migrations and before enforcement.
if [[ -n "${extra_sql}" ]]; then
  admin --dbname "${probe_db}" --file "${extra_sql}" >/dev/null
fi

# 4. ENFORCEMENT, on a database that now has the real schema. The require flag
#    turns "the census examined nothing" from a silent pass into a failure. It
#    is 1 unless the caller asked for the UNARMED configuration -- the one every
#    automated path runs BEFORE migrations -- which the census test uses to
#    measure the two controls the armed run's step 1c would otherwise mask.
enforcement_output=""
if ! enforcement_output="$(
      POSTGRES_DB="${probe_db}" \
      CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES="${require_tables}" \
      bash /topology.sh 2>&1
    )"; then
  printf '%s\n' "${enforcement_output}" >&2
  echo "canonical-enforce: FAILED on ${probe_db}" >&2
  admin --dbname "${POSTGRES_DB}" -c "DROP DATABASE IF EXISTS ${probe_db} WITH (FORCE)" >/dev/null
  exit 1
fi
printf '%s\n' "${enforcement_output}"

# The count is the block's OWN measurement, read back out of its NOTICE, so this
# line cannot claim more than the census actually looked at.
examined="$(printf '%s' "${enforcement_output}" \
  | sed -n 's/.*topology\.canonical_enforcement: examined \([0-9][0-9]*\) canonical tables.*/\1/p' \
  | tail -1)"
if [[ -z "${examined}" ]]; then
  echo "canonical-enforce: the reconcile did not report how many canonical tables it examined" >&2
  exit 1
fi
# The SET, not just the count: a rename shrinks the scope without emptying it,
# so the caller has to be able to see WHICH tables were enforced on.
examined_names="$(printf '%s' "${enforcement_output}" \
  | sed -n 's/.*topology\.canonical_enforcement: examined [0-9][0-9]* canonical tables \[\(.*\)\].*/\1/p' \
  | tail -1)"

admin --dbname "${POSTGRES_DB}" -c "DROP DATABASE IF EXISTS ${probe_db} WITH (FORCE)" >/dev/null
echo "canonical-enforce: enforced on ${probe_db}, examined ${examined} tables [${examined_names}]"
