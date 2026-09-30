#!/usr/bin/env bash
# Reconcile the portable eight-role application topology from a distinct cluster
# administrator. Safe to run on fresh or existing databases before migrations.
set -euo pipefail

: "${POSTGRES_HOST:=postgres}"
: "${POSTGRES_PORT:=5432}"
: "${POSTGRES_LOCAL_SOCKET_DIR:=/var/run/postgresql}"
: "${POSTGRES_DB:?required}"
: "${POSTGRES_ADMIN_USER:?required}"
: "${POSTGRES_ADMIN_PASSWORD:?required}"
: "${CONSOLE_APP_POSTGRES_PASSWORD:?required}"
: "${CONSOLE_RT_POSTGRES_PASSWORD:?required}"
: "${CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD:?required}"
: "${CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD:?required}"
: "${CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD:?required}"
: "${CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION:=0}"
# Set to 1 by the POST-MIGRATION invocation. The canonical writer-ownership
# census can only see the tables that exist when it runs, so a reconcile against
# a fresh cluster legitimately examines zero of them -- and a silently-skipped
# census must never be readable as a pass. With this set, examining zero
# canonical tables is a FAILURE, not a tolerated fresh database.
: "${CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES:=0}"
case "${CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES}" in
  0|1) ;;
  *)
    echo "topology: CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES must be 0 or 1" >&2
    exit 1
    ;;
esac
export POSTGRES_ADMIN_USER POSTGRES_ADMIN_PASSWORD CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES

if [[ "${POSTGRES_ADMIN_USER}" == "console_app" ]]; then
  echo "topology: POSTGRES_ADMIN_USER must be distinct from console_app" >&2
  exit 1
fi

passwords=(
  "${POSTGRES_ADMIN_PASSWORD}"
  "${CONSOLE_APP_POSTGRES_PASSWORD}"
  "${CONSOLE_RT_POSTGRES_PASSWORD}"
  "${CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD}"
  "${CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD}"
  "${CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD}"
)
for ((i = 0; i < ${#passwords[@]}; i++)); do
  for ((j = i + 1; j < ${#passwords[@]}; j++)); do
    if [[ "${passwords[i]}" == "${passwords[j]}" ]]; then
      echo "topology: cluster-admin, owner, runtime, and command passwords must be pairwise distinct" >&2
      exit 1
    fi
  done
done

admin_psql_args=(
  --host "${POSTGRES_HOST}"
  --port "${POSTGRES_PORT}"
  --username "${POSTGRES_ADMIN_USER}"
  --dbname "${POSTGRES_DB}"
  --set ON_ERROR_STOP=1
  --quiet
)
legacy_reassign_from_admin=0
legacy_conversion_admin_cleanup_armed=0

admin_connection_ready() {
  PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}" \
    psql "${admin_psql_args[@]}" -Atqc 'SELECT 1' >/dev/null 2>&1
}

neutralize_legacy_conversion_admin() {
  local neutralize_sql
  neutralize_sql="DO \$block\$ BEGIN IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname='console_legacy_conversion_admin') THEN ALTER ROLE console_legacy_conversion_admin NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD NULL; END IF; END \$block\$;"

  if admin_connection_ready; then
    PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}" \
      psql "${admin_psql_args[@]}" -c "${neutralize_sql}" >/dev/null
    return
  fi

  PGPASSWORD='' psql \
    --host "${POSTGRES_LOCAL_SOCKET_DIR}" \
    --port "${POSTGRES_PORT}" \
    --username console_app \
    --dbname "${POSTGRES_DB}" \
    --set ON_ERROR_STOP=1 \
    --quiet \
    -c "${neutralize_sql}" >/dev/null
}

cleanup_legacy_conversion_admin() {
  local status=$?
  if [[ "${legacy_conversion_admin_cleanup_armed}" == "1" ]]; then
    neutralize_legacy_conversion_admin || \
      echo "topology.legacy_conversion_admin_cleanup_failed: manual role neutralization is required" >&2
  fi
  return "${status}"
}

arm_legacy_conversion_admin_cleanup() {
  legacy_conversion_admin_cleanup_armed=1
  trap cleanup_legacy_conversion_admin EXIT
  trap 'exit 1' HUP INT TERM
}

bootstrap_legacy_admin() {
  if [[ "${CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION}" != "1" ]]; then
    echo "topology.admin_unavailable: the distinct cluster administrator could not connect; legacy conversion requires CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION=1" >&2
    exit 1
  fi
  if [[ ! -S "${POSTGRES_LOCAL_SOCKET_DIR}/.s.PGSQL.${POSTGRES_PORT}" ]]; then
    echo "topology.legacy_socket_unavailable: expected the PostgreSQL local socket at ${POSTGRES_LOCAL_SOCKET_DIR}" >&2
    exit 1
  fi

  local legacy_psql_args=(
    --host "${POSTGRES_LOCAL_SOCKET_DIR}"
    --port "${POSTGRES_PORT}"
    --username console_app
    --dbname "${POSTGRES_DB}"
    --set ON_ERROR_STOP=1
    --quiet
  )
  local legacy_identity
  legacy_identity="$(PGPASSWORD='' psql "${legacy_psql_args[@]}" -At -F '|' -c \
    "SELECT current_user, rolsuper FROM pg_roles WHERE rolname=current_user")"
  if [[ "${legacy_identity}" != "console_app|t" ]]; then
    echo "topology.legacy_identity_refused: local-socket bootstrap requires the extant console_app superuser" >&2
    exit 1
  fi

  arm_legacy_conversion_admin_cleanup
  neutralize_legacy_conversion_admin

  # The legacy escape hatch must prove the same transaction-timeout substrate
  # through the verified local-socket bootstrap identity before it creates or
  # renames any role. Keep the later administrator check as defense in depth.
  PGPASSWORD='' psql "${legacy_psql_args[@]}" <<'SQL'
DO $block$
BEGIN
  IF current_setting('server_version_num')::integer < 170000
     OR current_setting('max_prepared_transactions')::integer <> 0
     OR EXISTS (SELECT 1 FROM pg_prepared_xacts) THEN
    RAISE EXCEPTION 'topology.transaction_timeout_prerequisite_failed';
  END IF;
END
$block$;
SQL

  # Classify the legacy ACL before creating or renaming any role. A rejected
  # volume must retain its original console_app identity and ACL evidence exactly as
  # found so an operator can audit and repair it deliberately.
  local legacy_default_acl_state
  legacy_default_acl_state="$(PGPASSWORD='' psql "${legacy_psql_args[@]}" -At <<'SQL'
WITH relevant_defaults AS (
  SELECT defaults.defaclacl
  FROM pg_default_acl defaults
  WHERE defaults.defaclrole = (SELECT oid FROM pg_roles WHERE rolname=current_user)
    AND defaults.defaclnamespace = (SELECT oid FROM pg_namespace WHERE nspname='public')
    AND defaults.defaclobjtype = 'r'
), privileges AS (
  SELECT privilege.*
  FROM relevant_defaults defaults
  CROSS JOIN LATERAL aclexplode(defaults.defaclacl) privilege
)
SELECT CASE
  WHEN (SELECT count(*) FROM relevant_defaults) = 0 THEN 'absent'
  WHEN (SELECT count(*) FROM relevant_defaults) = 1
    AND (SELECT count(*) FROM privileges) = 4
    AND (
      SELECT count(*)
      FROM privileges
      WHERE grantee = (SELECT oid FROM pg_roles WHERE rolname='console_rt')
        AND privilege_type IN ('SELECT', 'INSERT', 'UPDATE', 'DELETE')
        AND NOT is_grantable
    ) = 4
    AND (SELECT count(DISTINCT privilege_type) FROM privileges) = 4
    THEN 'canonical'
  ELSE 'invalid'
END;
SQL
)"
  if [[ "${legacy_default_acl_state}" == "invalid" ]]; then
    echo "topology.legacy_default_acl_preflight_noncanonical: expected either no public table default ACL or exactly non-grantable SELECT, INSERT, UPDATE, DELETE for console_rt; original console_app and ACL preserved for audit" >&2
    exit 4
  fi

  # PostgreSQL 18 forbids removing SUPERUSER from the bootstrap role. The sole
  # legacy escape hatch therefore creates a temporary superuser locally, uses
  # it to rename the bootstrap console_app identity to the requested administrator,
  # then lets normal reconciliation recreate a non-superuser console_app and move
  # application ownership to it. Logging is suppressed before secret SQL.
  POSTGRES_ADMIN_PASSWORD="${POSTGRES_ADMIN_PASSWORD}" \
  PGPASSWORD='' psql "${legacy_psql_args[@]}" <<'SQL'
BEGIN;
SET LOCAL log_statement = 'none';
SET LOCAL log_min_error_statement = 'panic';
\getenv admin_password POSTGRES_ADMIN_PASSWORD
SELECT format(
  'CREATE ROLE console_legacy_conversion_admin LOGIN SUPERUSER PASSWORD %L',
  :'admin_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_legacy_conversion_admin') \gexec
SELECT format(
  'ALTER ROLE console_legacy_conversion_admin LOGIN SUPERUSER PASSWORD %L',
  :'admin_password'
) \gexec
COMMIT;
SQL

  local conversion_psql_args=(
    --host "${POSTGRES_HOST}"
    --port "${POSTGRES_PORT}"
    --username console_legacy_conversion_admin
    --dbname "${POSTGRES_DB}"
    --set ON_ERROR_STOP=1
    --quiet
  )
  PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}" psql "${conversion_psql_args[@]}" <<'SQL'
BEGIN;
SET LOCAL log_statement = 'none';
SET LOCAL log_min_error_statement = 'panic';
\getenv admin_user POSTGRES_ADMIN_USER
\getenv admin_password POSTGRES_ADMIN_PASSWORD
SELECT format('ALTER ROLE console_app RENAME TO %I', :'admin_user') \gexec
SELECT format(
  'ALTER ROLE %I LOGIN SUPERUSER CREATEDB CREATEROLE REPLICATION BYPASSRLS PASSWORD %L',
  :'admin_user', :'admin_password'
) \gexec
COMMIT;
SQL
  legacy_reassign_from_admin=1
  echo "topology: converted the legacy bootstrap identity into the distinct cluster administrator" >&2
}

if ! admin_connection_ready; then
  bootstrap_legacy_admin
fi
if ! admin_connection_ready; then
  echo "topology.admin_unavailable: distinct cluster administrator still cannot connect after bootstrap" >&2
  exit 1
fi

# A failed or interrupted conversion may leave this role behind. As soon as the
# real administrator is usable, revoke every login and elevated attribute and
# discard its password. The EXIT trap applies the same fail-closed cleanup via
# whichever administrator identity remains usable during bootstrap failures.
arm_legacy_conversion_admin_cleanup
neutralize_legacy_conversion_admin
legacy_conversion_admin_state="$(PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}" \
  psql "${admin_psql_args[@]}" -Atqc \
  "SELECT rolcanlogin::text || '|' || rolsuper::text || '|' || (rolpassword IS NULL)::text FROM pg_authid WHERE rolname='console_legacy_conversion_admin'")"
if [[ -n "${legacy_conversion_admin_state}" && "${legacy_conversion_admin_state}" != "false|false|true" ]]; then
  echo "topology.legacy_conversion_admin_neutralization_failed" >&2
  exit 1
fi
legacy_conversion_admin_cleanup_armed=0
trap - EXIT HUP INT TERM

export PGPASSWORD="${POSTGRES_ADMIN_PASSWORD}"
IFS='|' read -r current_user console_app_exists legacy_console_app_superuser conversion_role_exists < <(
  psql "${admin_psql_args[@]}" -At -F '|' -c \
    "SELECT current_user, EXISTS(SELECT 1 FROM pg_roles WHERE rolname='console_app'), COALESCE((SELECT rolsuper FROM pg_roles WHERE rolname='console_app'), false), EXISTS(SELECT 1 FROM pg_roles WHERE rolname='console_legacy_conversion_admin')"
)
if [[ "${current_user}" != "${POSTGRES_ADMIN_USER}" || "${current_user}" == "console_app" ]]; then
  echo "topology: connection did not resolve to the distinct cluster administrator" >&2
  exit 1
fi
if [[ "${legacy_console_app_superuser}" == "t" && "${CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION}" != "1" ]]; then
  echo "topology.legacy_console_app_superuser_refused: audit the volume, then set CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION=1 for one guarded reconciliation" >&2
  exit 1
fi
if [[ "${CONSOLE_ALLOW_LEGACY_CONSOLE_APP_SUPERUSER_CONVERSION}" == "1" && "${console_app_exists}" == "f" && "${conversion_role_exists}" == "t" ]]; then
  legacy_reassign_from_admin=1
fi
export CONSOLE_LEGACY_REASSIGN_FROM_ADMIN="${legacy_reassign_from_admin}"

# transaction_timeout is PostgreSQL 17+, and prepared transactions are exempt
# from it. Refuse the substrate before the normal topology transaction mutates
# application roles.
psql "${admin_psql_args[@]}" <<'SQL'
DO $block$
BEGIN
  IF current_setting('server_version_num')::integer < 170000
     OR current_setting('max_prepared_transactions')::integer <> 0
     OR EXISTS (SELECT 1 FROM pg_prepared_xacts) THEN
    RAISE EXCEPTION 'topology.transaction_timeout_prerequisite_failed';
  END IF;
END
$block$;
SQL

# Role passwords must be sent as SQL because PostgreSQL has no parameterized
# ALTER ROLE protocol. Suppress statement and error-statement logging for this
# privileged transaction before psql expands any password variables.
psql "${admin_psql_args[@]}" <<'SQL'
BEGIN;
SET LOCAL log_statement = 'none';
SET LOCAL log_min_error_statement = 'panic';
\getenv app_password CONSOLE_APP_POSTGRES_PASSWORD
\getenv runtime_password CONSOLE_RT_POSTGRES_PASSWORD
\getenv leave_password CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD
\getenv ontology_password CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD
\getenv platform_force_password CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD
\getenv legacy_reassign CONSOLE_LEGACY_REASSIGN_FROM_ADMIN
\getenv canonical_require CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES
-- Read back by the DO $canonical$ block below. A psql variable cannot be
-- interpolated inside a dollar-quoted body, so it travels as a GUC.
SET LOCAL console.canonical_require_tables = :'canonical_require';

SELECT format(
  'CREATE ROLE console_app LOGIN NOSUPERUSER BYPASSRLS INHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'app_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_app') \gexec
SELECT format(
  'ALTER ROLE console_app LOGIN NOSUPERUSER BYPASSRLS INHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'app_password'
) \gexec

SELECT format(
  'CREATE ROLE console_rt LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'runtime_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_rt') \gexec
SELECT format(
  'ALTER ROLE console_rt LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'runtime_password'
) \gexec

SELECT format(
  'CREATE ROLE console_leave_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'leave_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_leave_cmd') \gexec
SELECT format(
  'ALTER ROLE console_leave_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'leave_password'
) \gexec

SELECT format(
  'CREATE ROLE console_ontology_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'ontology_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_ontology_cmd') \gexec
SELECT format(
  'ALTER ROLE console_ontology_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'ontology_password'
) \gexec

SELECT format(
  'CREATE ROLE console_platform_force_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'platform_force_password'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_platform_force_cmd') \gexec
SELECT format(
  'ALTER ROLE console_platform_force_cmd LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD %L',
  :'platform_force_password'
) \gexec

-- Expand-only authentication owner. It has no login/password or caller until
-- the reviewed proof/admission cutover supplies a separate credential.
SELECT 'CREATE ROLE console_auth_cmd NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD NULL'
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_auth_cmd') \gexec
ALTER ROLE console_auth_cmd NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD NULL;

-- Bound every transaction capable of writing through a serving connection.
-- Database-specific settings outrank global role defaults, so remove only the
-- three managed keys from every database override and preserve all unrelated
-- role settings.
ALTER ROLE console_rt SET statement_timeout = '30s';
ALTER ROLE console_rt SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE console_rt SET transaction_timeout = '45s';
ALTER ROLE console_leave_cmd SET statement_timeout = '30s';
ALTER ROLE console_leave_cmd SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE console_leave_cmd SET transaction_timeout = '45s';
ALTER ROLE console_ontology_cmd SET statement_timeout = '30s';
ALTER ROLE console_ontology_cmd SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE console_ontology_cmd SET transaction_timeout = '45s';
ALTER ROLE console_platform_force_cmd SET statement_timeout = '30s';
ALTER ROLE console_platform_force_cmd SET idle_in_transaction_session_timeout = '30s';
ALTER ROLE console_platform_force_cmd SET transaction_timeout = '45s';
SELECT format('ALTER ROLE %I IN DATABASE %I RESET statement_timeout', role.rolname, database.datname)
FROM pg_db_role_setting settings
JOIN pg_roles role ON role.oid = settings.setrole
JOIN pg_database database ON database.oid = settings.setdatabase
WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd')
  AND EXISTS (SELECT 1 FROM unnest(settings.setconfig) setting WHERE setting LIKE 'statement_timeout=%')
\gexec
SELECT format('ALTER ROLE %I IN DATABASE %I RESET idle_in_transaction_session_timeout', role.rolname, database.datname)
FROM pg_db_role_setting settings
JOIN pg_roles role ON role.oid = settings.setrole
JOIN pg_database database ON database.oid = settings.setdatabase
WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd')
  AND EXISTS (SELECT 1 FROM unnest(settings.setconfig) setting WHERE setting LIKE 'idle_in_transaction_session_timeout=%')
\gexec
SELECT format('ALTER ROLE %I IN DATABASE %I RESET transaction_timeout', role.rolname, database.datname)
FROM pg_db_role_setting settings
JOIN pg_roles role ON role.oid = settings.setrole
JOIN pg_database database ON database.oid = settings.setdatabase
WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd')
  AND EXISTS (SELECT 1 FROM unnest(settings.setconfig) setting WHERE setting LIKE 'transaction_timeout=%')
\gexec

SELECT format(
  'CREATE ROLE console_leave_definer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_leave_definer') \gexec
ALTER ROLE console_leave_definer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION;

SELECT format(
  'CREATE ROLE console_ontology_writer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION'
) WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'console_ontology_writer') \gexec
ALTER ROLE console_ontology_writer NOLOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE NOREPLICATION;

-- Migration 0031 owns fresh-database timing. A guarded legacy rename may leave
-- its table default ACL attached to the renamed bootstrap administrator OID.
-- Skip only when that legacy table ACL is absent, transfer only its exact known
-- shape, and fail closed without changing a noncanonical ACL. Do not introduce
-- sequence/function defaults.
WITH relevant_defaults AS (
  SELECT defaults.defaclacl
  FROM pg_default_acl defaults
  WHERE defaults.defaclrole = (SELECT oid FROM pg_roles WHERE rolname=current_user)
    AND defaults.defaclnamespace = (SELECT oid FROM pg_namespace WHERE nspname='public')
    AND defaults.defaclobjtype = 'r'
), privileges AS (
  SELECT privilege.*
  FROM relevant_defaults defaults
  CROSS JOIN LATERAL aclexplode(defaults.defaclacl) privilege
), classified AS (
  SELECT CASE
    WHEN :'legacy_reassign' <> '1' THEN 'skip'
    WHEN (SELECT count(*) FROM relevant_defaults) = 0 THEN 'absent'
    WHEN (SELECT count(*) FROM relevant_defaults) = 1
      AND (SELECT count(*) FROM privileges) = 4
      AND (
        SELECT count(*)
        FROM privileges
        WHERE grantee = (SELECT oid FROM pg_roles WHERE rolname='console_rt')
          AND privilege_type IN ('SELECT', 'INSERT', 'UPDATE', 'DELETE')
          AND NOT is_grantable
      ) = 4
      AND (SELECT count(DISTINCT privilege_type) FROM privileges) = 4
      THEN 'canonical'
    ELSE 'invalid'
  END AS state
)
SELECT state AS legacy_default_acl_state,
       (state = 'invalid')::text AS legacy_default_acl_invalid
FROM classified \gset
\if :legacy_default_acl_invalid
  \echo topology.legacy_default_acl_noncanonical: expected either no public table default ACL or exactly non-grantable SELECT, INSERT, UPDATE, DELETE for console_rt; preserved legacy ACL for audit
  SELECT 'topology.legacy_default_acl_noncanonical'::integer;
\endif
SELECT 'ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt'
WHERE :'legacy_default_acl_state' = 'canonical' \gexec
SELECT format(
  'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE SELECT, INSERT, UPDATE, DELETE ON TABLES FROM console_rt',
  current_user
)
WHERE :'legacy_default_acl_state' = 'canonical' \gexec
SELECT CASE WHEN :'legacy_default_acl_state' <> 'canonical' OR (
  EXISTS (
    SELECT 1
    FROM pg_default_acl defaults
    WHERE defaults.defaclrole = (SELECT oid FROM pg_roles WHERE rolname='console_app')
      AND defaults.defaclnamespace = (SELECT oid FROM pg_namespace WHERE nspname='public')
      AND defaults.defaclobjtype = 'r'
      AND (
        SELECT count(*)
        FROM aclexplode(defaults.defaclacl) privilege
        WHERE privilege.grantee = (SELECT oid FROM pg_roles WHERE rolname='console_rt')
          AND privilege.privilege_type IN ('SELECT', 'INSERT', 'UPDATE', 'DELETE')
          AND NOT privilege.is_grantable
      ) = 4
  )
  AND NOT EXISTS (
    SELECT 1
    FROM pg_default_acl defaults
    CROSS JOIN LATERAL aclexplode(defaults.defaclacl) privilege
    WHERE defaults.defaclrole = (SELECT oid FROM pg_roles WHERE rolname=current_user)
      AND defaults.defaclnamespace = (SELECT oid FROM pg_namespace WHERE nspname='public')
      AND defaults.defaclobjtype = 'r'
      AND privilege.grantee = (SELECT oid FROM pg_roles WHERE rolname='console_rt')
  )
) THEN 'true' ELSE 'false' END AS legacy_default_acl_repaired \gset
\if :legacy_default_acl_repaired
\else
  \echo topology.legacy_default_acl_repair_failed
  SELECT 'topology.legacy_default_acl_repair_failed'::integer;
\endif

-- After the PostgreSQL-18 legacy rename, move user-schema application objects
-- from the renamed bootstrap administrator to the recreated console_app. A blanket
-- REASSIGN OWNED is forbidden for the bootstrap role because it owns objects
-- required by the database system, so enumerate only portable user objects.
SELECT format(
  'ALTER %s %I.%I OWNER TO console_app',
  CASE relation.relkind
    WHEN 'S' THEN 'SEQUENCE'
    WHEN 'v' THEN 'VIEW'
    WHEN 'm' THEN 'MATERIALIZED VIEW'
    WHEN 'f' THEN 'FOREIGN TABLE'
    ELSE 'TABLE'
  END,
  namespace.nspname,
  relation.relname
)
FROM pg_class relation
JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
WHERE :'legacy_reassign' = '1'
  AND relation.relowner = (SELECT oid FROM pg_roles WHERE rolname=current_user)
  AND relation.relkind IN ('r', 'p', 'S', 'v', 'm', 'f')
  AND namespace.nspname <> 'information_schema'
  AND namespace.nspname NOT LIKE 'pg\_%' ESCAPE '\'
\gexec
SELECT format(
  'ALTER %s %I.%I(%s) OWNER TO console_app',
  CASE routine.prokind WHEN 'p' THEN 'PROCEDURE' WHEN 'a' THEN 'AGGREGATE' ELSE 'FUNCTION' END,
  namespace.nspname,
  routine.proname,
  pg_get_function_identity_arguments(routine.oid)
)
FROM pg_proc routine
JOIN pg_namespace namespace ON namespace.oid = routine.pronamespace
WHERE :'legacy_reassign' = '1'
  AND routine.proowner = (SELECT oid FROM pg_roles WHERE rolname=current_user)
  AND namespace.nspname <> 'information_schema'
  AND namespace.nspname NOT LIKE 'pg\_%' ESCAPE '\'
\gexec
SELECT format(
  'ALTER %s %I.%I OWNER TO console_app',
  CASE type.typtype WHEN 'd' THEN 'DOMAIN' ELSE 'TYPE' END,
  namespace.nspname,
  type.typname
)
FROM pg_type type
JOIN pg_namespace namespace ON namespace.oid = type.typnamespace
WHERE :'legacy_reassign' = '1'
  AND type.typowner = (SELECT oid FROM pg_roles WHERE rolname=current_user)
  AND type.typrelid = 0
  AND namespace.nspname <> 'information_schema'
  AND namespace.nspname NOT LIKE 'pg\_%' ESCAPE '\'
\gexec
SELECT format('ALTER LARGE OBJECT %s OWNER TO console_app', oid)
FROM pg_largeobject_metadata
WHERE :'legacy_reassign' = '1'
  AND lomowner = (SELECT oid FROM pg_roles WHERE rolname=current_user)
\gexec
SELECT format('ALTER SCHEMA %I OWNER TO console_app', nspname)
FROM pg_namespace
WHERE :'legacy_reassign' = '1'
  AND nspowner = (SELECT oid FROM pg_roles WHERE rolname=current_user)
  AND nspname <> 'information_schema'
  AND nspname NOT LIKE 'pg\_%' ESCAPE '\'
\gexec

-- Exact topology: remove every membership edge touching any application role,
-- including edges to or from an unexpected external role, then restore only
-- the two explicitly allowed non-admin memberships.
SELECT format('REVOKE %I FROM %I', granted.rolname, member.rolname)
FROM pg_auth_members membership
JOIN pg_roles member ON member.oid = membership.member
JOIN pg_roles granted ON granted.oid = membership.roleid
WHERE member.rolname IN (
        'console_app', 'console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd', 'console_auth_cmd',
        'console_leave_definer', 'console_ontology_writer'
      )
   OR granted.rolname IN (
        'console_app', 'console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd', 'console_auth_cmd',
        'console_leave_definer', 'console_ontology_writer'
      )
\gexec
GRANT console_leave_definer, console_ontology_writer TO console_app
    WITH ADMIN FALSE, INHERIT TRUE, SET TRUE;

SELECT format('ALTER DATABASE %I OWNER TO console_app', current_database()) \gexec
ALTER SCHEMA public OWNER TO console_app;

DO $block$
DECLARE
    bad_roles INTEGER;
    bad_memberships INTEGER;
    bad_runtime_defaults INTEGER;
BEGIN
    IF current_setting('server_version_num')::integer < 170000
       OR current_setting('max_prepared_transactions')::integer <> 0
       OR EXISTS (SELECT 1 FROM pg_prepared_xacts) THEN
        RAISE EXCEPTION 'topology.transaction_timeout_prerequisite_failed';
    END IF;
    SELECT count(*) INTO bad_roles
    FROM pg_roles
    WHERE (rolname = 'console_app' AND (NOT rolcanlogin OR rolsuper OR NOT rolbypassrls OR NOT rolinherit OR rolcreatedb OR rolcreaterole OR rolreplication))
       OR (rolname = 'console_rt' AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit OR rolcreatedb OR rolcreaterole OR rolreplication))
       OR (rolname IN ('console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd') AND (NOT rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit OR rolcreatedb OR rolcreaterole OR rolreplication))
       OR (rolname IN ('console_leave_definer', 'console_ontology_writer', 'console_auth_cmd') AND (rolcanlogin OR rolsuper OR rolbypassrls OR rolinherit OR rolcreatedb OR rolcreaterole OR rolreplication));
    IF bad_roles <> 0 OR (SELECT count(*) FROM pg_roles WHERE rolname IN (
        'console_app', 'console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd', 'console_auth_cmd',
        'console_leave_definer', 'console_ontology_writer'
    )) <> 8 THEN
        RAISE EXCEPTION 'topology.role_readback_failed';
    END IF;

    SELECT count(*) INTO bad_memberships
    FROM pg_auth_members membership
    JOIN pg_roles member ON member.oid = membership.member
    JOIN pg_roles granted ON granted.oid = membership.roleid
    WHERE (
        member.rolname IN (
          'console_app', 'console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd', 'console_auth_cmd',
          'console_leave_definer', 'console_ontology_writer'
        )
        OR granted.rolname IN (
          'console_app', 'console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd', 'console_auth_cmd',
          'console_leave_definer', 'console_ontology_writer'
        )
      )
      AND NOT (
        member.rolname = 'console_app'
        AND granted.rolname IN ('console_leave_definer', 'console_ontology_writer')
        AND NOT membership.admin_option
        AND membership.inherit_option
        AND membership.set_option
      );
    IF bad_memberships <> 0 OR (
        SELECT count(*)
        FROM pg_auth_members membership
        JOIN pg_roles member ON member.oid = membership.member
        JOIN pg_roles granted ON granted.oid = membership.roleid
        WHERE member.rolname = 'console_app'
          AND granted.rolname IN ('console_leave_definer', 'console_ontology_writer')
          AND NOT membership.admin_option
          AND membership.inherit_option
          AND membership.set_option
    ) <> 2 THEN
        RAISE EXCEPTION 'topology.membership_readback_failed';
    END IF;
    IF (SELECT pg_get_userbyid(datdba) FROM pg_database WHERE datname=current_database()) <> 'console_app' THEN
        RAISE EXCEPTION 'topology.database_owner_readback_failed';
    END IF;

    SELECT count(*) INTO bad_runtime_defaults
    FROM (VALUES
      ('console_rt'), ('console_leave_cmd'), ('console_ontology_cmd'), ('console_platform_force_cmd')
    ) expected(role_name)
    WHERE NOT EXISTS (
      SELECT 1
      FROM pg_db_role_setting settings
      JOIN pg_roles role ON role.oid = settings.setrole
      WHERE role.rolname = expected.role_name
        AND settings.setdatabase = 0
        AND settings.setconfig @> ARRAY[
          'statement_timeout=30s',
          'idle_in_transaction_session_timeout=30s',
          'transaction_timeout=45s'
        ]
    );
    IF bad_runtime_defaults <> 0 OR EXISTS (
      SELECT 1
      FROM pg_db_role_setting settings
      JOIN pg_roles role ON role.oid = settings.setrole
      CROSS JOIN LATERAL unnest(settings.setconfig) setting
      WHERE role.rolname IN ('console_rt', 'console_leave_cmd', 'console_ontology_cmd', 'console_platform_force_cmd')
        AND settings.setdatabase <> 0
        AND split_part(setting, '=', 1) IN (
          'statement_timeout', 'idle_in_transaction_session_timeout', 'transaction_timeout'
        )
    ) THEN
        RAISE EXCEPTION 'topology.runtime_default_readback_failed';
    END IF;
END
$block$;

-- Canonical-object writer ownership: the DATABASE half.
--
-- WHAT THIS HALF IS TOTAL OVER: whether a ROLE holds a DML privilege ON a
-- relation in the reachable canonical set. It does not union catalogs and it
-- keeps no list of the ways a privilege can arrive. It asks PostgreSQL the
-- question directly, per role, per relation, per DML verb --
-- `has_any_column_privilege`/`has_table_privilege` already account for direct
-- grants, COLUMN grants, grants to PUBLIC, OWNERSHIP, role membership and
-- superuser in one call, including the sources nobody has thought of yet. Four
-- rounds were lost to unioning relacl, then attacl, then relowner, then
-- pg_auth_members, then rolsuper, then partition children; each round closed the
-- named case and the next reviewer named another. That enumeration is gone.
--
-- WHAT IT IS NOT TOTAL OVER: the PATHS by which canonical rows get written. The
-- privilege question is asked about the relations in the examined set, so a role
-- that writes canonical rows through some OTHER object holds no privilege the
-- census can see. Two live examples, each executed against a migrated database
-- by review and observed passing this block:
--
--   * an auto-updatable VIEW over a canonical table, under a name the roster
--     does not carry, granted to a rogue;
--   * a SECURITY DEFINER function owned by a privileged role -- which is how
--     `lose_dml` roles are documented to reach their data, so this is the
--     architecture rather than a corner case.
--
-- Closing those needs a census over view dependencies and function bodies. It is
-- follow-up work; it is stated here rather than implied away.
--
-- Two consequences of asking PostgreSQL the privilege question, embraced rather
-- than worked around:
--
--   * A SUPERUSER reports true on everything, so every superuser must appear on
--     the expected-writer ratchet BY NAME. That is the point: the trusted set is
--     explicit instead of silently excluded, which is what let an unexpected
--     superuser write every canonical table and still pass. The single exception
--     is the role EXECUTING this reconcile (`session_user`) -- it is running the
--     block, so trusting it is not a decision the block gets to make.
--   * MEMBERSHIP stops being a separate guard. `pg_write_all_data` holds DML in
--     every cluster by construction and is named on the ratchet, so anyone who
--     can BECOME it fails; that is what the deleted pg_auth_members guard was
--     reaching for.
--
-- OWNERSHIP does NOT collapse into it, and folding it in was a fail-open. The
-- census excludes a candidate by NAME before it asks the privilege question, so
-- every role on the write ratchet -- including `console_rt`, the runtime login
-- principal -- could OWN a canonical table and report clean. An owner holds more
-- than DML: ALTER, DROP, TRUNCATE, and `DISABLE ROW LEVEL SECURITY` / `DROP
-- POLICY` on relations whose tenant isolation IS row-level security. So the
-- owner is pinned separately by `expected_owners`, which the ratchet cannot
-- widen.
--
-- WHAT IT CANNOT DO: draw a CRATE boundary. deploy/apps/console/base/backend.yaml:62
-- and worker.yaml:53 both connect the runtime as console_rt, and the ratchet
-- whitelists ('console_rt','*'). Every crate in the application is therefore the
-- same database principal: this half cannot tell console-payroll-adapter-postgres
-- from console-leave-adapter-postgres. The static gate in
-- backend/ci/gates/writer-ownership is the ONLY crate-level boundary that exists
-- today. Per-crate database roles are what would make this half crate-aware;
-- they are follow-up work, not done here.
--
-- WHEN IT RUNS: on every reconcile, on the relations that exist right now. That
-- is why CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES exists. Every automated path
-- ran the reconcile BEFORE migrations, so the REVOKE loop iterated nothing and
-- the census returned NULL unconditionally -- a structural no-op that looked
-- like a pass. The post-migration invocation
-- (backend/ci/gates/writer-ownership/canonical-enforce.sh) sets the flag, and
-- then examining zero canonical relations is a FAILURE.
--
-- It does four things:
--
--   1. RESOLVES the roster by REACHABILITY, not by name-in-public. Each roster
--      name must resolve to exactly one non-temporary relation, in `public`, as
--      a table or partitioned table; a name that has moved schema, changed
--      relkind, or acquired a second copy anywhere in the cluster (a shadow
--      copy is a copy outside `public`) FAILS instead of quietly dropping out of
--      the examined set. The resolved roots are then expanded through
--      pg_inherits in BOTH directions -- to every partition and inheritance
--      CHILD, because a grant on a child is a write to the parent's rows, and to
--      every PARENT a canonical table has been made a child of, because DML on
--      that parent writes the canonical rows while holding no privilege on the
--      canonical relation at all. Neither carries a roster relname. A migrated
--      database must additionally have every `required_tables` entry, and
--      examining zero relations while claiming to enforce is a failure.
--   2. REVOKES INSERT, UPDATE, DELETE, TRUNCATE on every relation in that set
--      from the three command roles below. They reach their data through
--      SECURITY DEFINER functions; direct table DML is never part of their job.
--   3. CENSUSES the full cross product of (every role that is not on the ratchet)
--      x (every relation in that set) x (INSERT, UPDATE, DELETE, TRUNCATE), and
--      RAISES on any that can write. Deny by default: a role nobody thought of,
--      a grant to PUBLIC, a rogue owner, a rogue superuser and a member of a
--      write role are all the same finding.
--   4. PINS the OWNER of every relation in that set to `expected_owners`,
--      independently of the census ratchet. Ownership is DDL authority, not
--      DML, so a role the ratchet permits to WRITE may still not OWN.
--
-- Membership is asked as reachability too. `has_table_privilege` ignores a
-- NOINHERIT membership, but a NOINHERIT member can still SET ROLE and write, so
-- the census asks whether the candidate can BECOME any role that holds the
-- privilege -- `pg_has_role(candidate, holder, 'MEMBER')`, which is reflexive
-- and therefore subsumes the direct question.
--
-- The membership-readback control at the exact-topology REVOKE above is a
-- DIFFERENT control and stays: it REPAIRS membership in the seven application
-- roles before this block runs, and `topology.membership_readback_failed` pins
-- the result.
--
-- WHICH ROLE LOSES WHICH DML, and what is still expected:
--   console_leave_cmd,
--   console_ontology_cmd,
--   console_platform_force_cmd,
--   console_auth_cmd             lose INSERT, UPDATE, DELETE, TRUNCATE on every
--                               relation in the resolved set, every reconcile.
--   console_app                 OWNS every canonical table (migrations run as
--                               it) and therefore holds implicit DML. Named on
--                               the write ratchet, and the sole entry in
--                               `expected_owners`.
--   console_rt                  still holds INSERT, UPDATE, DELETE on all of
--                               them, from the ALTER DEFAULT PRIVILEGES above.
--                               The port lanes remove it.
--   console_leave_definer       still holds INSERT, UPDATE on `employees`, from
--                               migration 0166. console-kmb removes it.
--   pg_write_all_data           holds DML on every table in every PostgreSQL
--                               cluster by construction. Naming the ROLE is what
--                               makes every MEMBER of it fail.
--   session_user                the role executing this reconcile.
--   anything else               fails the reconcile.
--
-- The expected list is a RATCHET: it is the measured writer surface of this
-- tree, each entry carries the lane that deletes it, and no lane may add one
-- without editing this file and the guard test that pins it.
DO $canonical$
DECLARE
    canonical_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN table roster. Kept identical to
      -- console_ontology_canonical_domain::ObjectKey::owned_tables; the test
      -- `topology_script_table_roster_matches_the_registry` in
      -- backend/ci/gates/writer-ownership asserts the two lists are equal.
      'organizations',
      'company_revisions',
      'org_units',
      'org_unit_revisions',
      'org_unit_source_bindings',
      'job_positions',
      'job_position_revisions',
      'persons',
      'person_revisions',
      'employee_person_bindings',
      'employees',
      'employment_heads',
      'employment_revisions',
      'employment_source_bindings',
      'payroll_draft_runs',
      'payroll_draft_lines',
      'payroll_line_calculations',
      'payroll_run_exceptions',
      'payroll_disbursements',
      'payroll_payslip_deliveries'
      -- canonical-writer-ownership: END table roster
    ];
    required_tables CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN required tables. The roster entries a
      -- MIGRATED database must actually have, so that the census's SCOPE is
      -- pinned and not merely non-empty. As of migration 0215 this list is the
      -- WHOLE roster above -- every canonical name now resolves to a real
      -- relation, so there is no longer such a thing as a roster name a
      -- migrated database may legitimately lack. The executed census test pins
      -- this exact set.
      'organizations',
      'employees',
      -- Migration 0213: the three tables of ObjectKey::Person.
      'persons',
      'person_revisions',
      'employee_person_bindings',
      -- Migration 0214: the three MISSING tables of ObjectKey::Employment.
      -- 'employees' is already listed above and is not re-added.
      'employment_heads',
      'employment_revisions',
      'employment_source_bindings',
      -- Migration 0215: the last six. 'organizations' is the seventh name of
      -- these three objects, already listed above and not re-added.
      'company_revisions',
      'org_units',
      'org_unit_revisions',
      'org_unit_source_bindings',
      'job_positions',
      'job_position_revisions',
      'payroll_draft_runs',
      'payroll_draft_lines',
      'payroll_line_calculations',
      'payroll_run_exceptions',
      'payroll_disbursements',
      'payroll_payslip_deliveries'
      -- canonical-writer-ownership: END required tables
    ];
    expected_owners CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN expected owners. Migrations are
      -- applied as console_app, so it owns every canonical relation. This is
      -- NOT the expected-writer ratchet below and no entry there may widen it:
      -- ownership is DDL authority (ALTER, DROP, TRUNCATE, DISABLE ROW LEVEL
      -- SECURITY, DROP POLICY), not the DML the census asks about.
      'console_app'
      -- canonical-writer-ownership: END expected owners
    ];
    lose_dml CONSTANT TEXT[] := ARRAY[
      -- canonical-writer-ownership: BEGIN revoked roles
      'console_leave_cmd',
      'console_ontology_cmd',
      'console_platform_force_cmd',
      'console_auth_cmd'
      -- canonical-writer-ownership: END revoked roles
    ];
    dml CONSTANT TEXT[] := ARRAY['INSERT', 'UPDATE', 'DELETE', 'TRUNCATE'];
    require_tables CONSTANT BOOLEAN :=
        COALESCE(current_setting('console.canonical_require_tables', true), '0') = '1';
    examined OID[];
    examined_names TEXT;
    revoke_targets TEXT;
    target TEXT;
    offending TEXT;
    leaked TEXT;
BEGIN
    -- 1a. RESOLUTION. A roster name is a name; a rename, a schema move, a view
    -- swap or a shadow copy all break name matching, and every one of them
    -- shrinks the examined set without emptying it. So resolve the name and
    -- FAIL on anything that is not exactly one table in `public`. Temporary
    -- relations are excluded because a temp schema is session-private and
    -- cannot alias a shared write.
    SELECT string_agg(problem, ', ' ORDER BY problem)
    INTO offending
    FROM (
        SELECT wanted.name || ' -> ' || string_agg(
                 namespace.nspname || '.' || relation.relname || ':' || relation.relkind::TEXT,
                 '+' ORDER BY namespace.nspname, relation.relname
               ) AS problem
        FROM unnest(canonical_tables) AS wanted(name)
        JOIN pg_class relation
          ON relation.relname = wanted.name
         AND relation.relpersistence <> 't'
        JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
        GROUP BY wanted.name
        HAVING bool_or(namespace.nspname <> 'public')
            OR bool_or(relation.relkind NOT IN ('r', 'p'))
    ) AS unresolved;
    IF offending IS NOT NULL THEN
        RAISE EXCEPTION 'topology.canonical_roster_unresolved: a canonical name does not resolve to exactly one public table, so the enforced set shrank: %', offending;
    END IF;

    -- 1b. REACHABILITY. The resolved roots plus every relation an inheritance
    -- edge connects them to, in BOTH directions. Neither carries a roster
    -- relname, so a name-matched census cannot see a grant on either.
    WITH RECURSIVE roots AS (
        SELECT relation.oid
        FROM pg_class relation
        WHERE relation.relnamespace = 'public'::regnamespace
          AND relation.relkind IN ('r', 'p')
          AND relation.relpersistence <> 't'
          AND relation.relname = ANY (canonical_tables)
    ), reachable AS (
        SELECT roots.oid FROM roots
        UNION
        SELECT edge.reached
        FROM reachable
        -- One recursive reference, so both directions travel as edges of one
        -- relation. PostgreSQL rejects a WITH RECURSIVE whose recursive term
        -- names itself twice.
        JOIN (
            -- DOWN: rows written through a partition or inheritance CHILD land
            -- in the parent, and the child carries a relname the roster has not.
            SELECT inheritance.inhparent AS held, inheritance.inhrelid AS reached
            FROM pg_inherits inheritance
            UNION ALL
            -- UP: a canonical table made the CHILD of some other relation is
            -- written through that PARENT, which needs no privilege on the
            -- canonical relation at all -- so a census scoped to the roster's
            -- own relations asks a question that cannot see the write.
            SELECT inheritance.inhrelid AS held, inheritance.inhparent AS reached
            FROM pg_inherits inheritance
        ) AS edge ON edge.held = reachable.oid
    )
    SELECT array_agg(relation.oid ORDER BY relation.relname),
           string_agg(relation.relname, ',' ORDER BY relation.relname)
    INTO examined, examined_names
    FROM reachable
    JOIN pg_class relation ON relation.oid = reachable.oid;

    RAISE NOTICE 'topology.canonical_enforcement: examined % canonical tables [%]',
        COALESCE(cardinality(examined), 0), COALESCE(examined_names, '');
    IF COALESCE(cardinality(examined), 0) = 0 AND require_tables THEN
        RAISE EXCEPTION 'topology.canonical_enforcement_examined_no_tables: this run claimed to enforce canonical writer ownership and found no canonical table to enforce it on';
    END IF;

    -- 1c. The roster entries a MIGRATED database must have. Armed by the same
    -- GUC as the zero-relation guard, because a pre-migration reconcile
    -- legitimately has none of these.
    IF require_tables THEN
        SELECT string_agg(wanted.name, ', ' ORDER BY wanted.name)
        INTO offending
        FROM unnest(required_tables) AS wanted(name)
        WHERE NOT EXISTS (
            SELECT 1
            FROM pg_class relation
            WHERE relation.relnamespace = 'public'::regnamespace
              AND relation.relkind IN ('r', 'p')
              AND relation.relname = wanted.name
        );
        IF offending IS NOT NULL THEN
            RAISE EXCEPTION 'topology.canonical_roster_incomplete: a canonical table this run must enforce on is missing, so the census silently shrank: %', offending;
        END IF;
    END IF;

    -- 2. REVOKE, on the same resolved set. `regclass` renders each relation
    -- schema-qualified and quoted as needed, so a partition child is revoked by
    -- its own identity rather than by the roster name it does not carry.
    SELECT string_agg(quote_ident(role_name), ', ' ORDER BY role_name)
    INTO revoke_targets
    FROM unnest(lose_dml) AS wanted(role_name)
    WHERE EXISTS (SELECT 1 FROM pg_roles WHERE rolname = wanted.role_name);
    IF revoke_targets IS NOT NULL THEN
        FOR target IN
            SELECT relation.oid::regclass::TEXT
            FROM unnest(examined) AS relation(oid)
            ORDER BY 1
        LOOP
            EXECUTE format(
                'REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON %s FROM %s',
                target,
                revoke_targets
            );
        END LOOP;
    END IF;

    -- 3. THE CENSUS. One question, asked of PostgreSQL, for every unratcheted
    -- role x every examined relation x every DML verb.
    --
    -- canonical-writer-ownership: BEGIN census statement. The region
    -- `topology_script_names_the_roles_that_lose_dml` holds the deleted-catalog
    -- ratchet over: the six catalog columns and views that guard test names may
    -- not be read HERE. Each was a hand-maintained union that
    -- `has_table_privilege`/`has_any_column_privilege` subsume, and the OWNER
    -- column in particular becomes subject to the ratchet below when it is read
    -- here, which is the proven fail-open the step-4 owner pin exists to close.
    SELECT string_agg(DISTINCT census.entry, ', ')
    INTO leaked
    FROM (
        SELECT format('%s:%s:%s', candidate.rolname, relation.relname, privilege.name) AS entry
        FROM pg_roles candidate
        CROSS JOIN unnest(examined) AS examined_relation(oid)
        JOIN pg_class relation ON relation.oid = examined_relation.oid
        CROSS JOIN unnest(dml) AS privilege(name)
        WHERE NOT EXISTS (
              SELECT 1
              FROM (VALUES
                -- canonical-writer-ownership: BEGIN expected writers.
                -- (role, relation name or '*' for every examined relation).
                -- Pinned by `topology_script_names_the_roles_that_lose_dml`;
                -- each entry names the lane that deletes it.
                -- console_app: owner of every canonical table, because
                -- migrations are applied as it, and therefore a holder of
                -- implicit DML. This entry authorises the WRITE only; who may
                -- OWN is `expected_owners`, which no entry here can widen.
                ('console_app', '*'),
                -- console_rt: ALTER DEFAULT PRIVILEGES above. Deleted by the six
                -- port lanes, which route runtime writes through the ports.
                ('console_rt', '*'),
                -- console_leave_definer: migration 0166 GRANT INSERT, UPDATE ON
                -- public.employees. Deleted by console-kmb (EmploymentPort).
                ('console_leave_definer', 'employees'),
                -- pg_write_all_data: a PostgreSQL predefined role that holds DML
                -- on every table in every cluster. Nothing deletes it; naming
                -- the role is what makes every MEMBER of it fail below.
                ('pg_write_all_data', '*')
                -- canonical-writer-ownership: END expected writers
              ) AS expected(role_name, table_name)
              WHERE expected.role_name = candidate.rolname
                AND expected.table_name IN ('*', relation.relname)
          )
          -- The role EXECUTING this reconcile. It is the cluster admin by
          -- construction and is running this block; every OTHER superuser is a
          -- finding, which is the hole this replaces.
          AND candidate.rolname <> session_user
          AND EXISTS (
              SELECT 1
              FROM pg_roles holder
              WHERE pg_has_role(candidate.oid, holder.oid, 'MEMBER')
                AND CASE
                      -- INSERT and UPDATE are the only DML privileges
                      -- PostgreSQL lets a grant scope to a COLUMN, and
                      -- `has_table_privilege` answers false for a column-only
                      -- grant. `has_any_column_privilege` answers the table
                      -- question too, so this is strictly wider, never narrower.
                      WHEN privilege.name IN ('INSERT', 'UPDATE')
                        THEN has_any_column_privilege(holder.oid, relation.oid, privilege.name)
                      ELSE has_table_privilege(holder.oid, relation.oid, privilege.name)
                    END
          )
    ) AS census;
    -- canonical-writer-ownership: END census statement
    IF leaked IS NOT NULL THEN
        RAISE EXCEPTION 'topology.canonical_writer_ownership_failed: unaccounted DML on a canonical table: %', leaked;
    END IF;

    -- 4. THE OWNER PIN, asked separately from the census and NOT subject to its
    -- ratchet.
    --
    -- The census answers "may this ROLE write this relation", and its ratchet is
    -- a (role, relation) whitelist of the live WRITE holders. Ownership is a
    -- different and strictly larger authority: ALTER, DROP, TRUNCATE, and -- for
    -- relations whose tenant isolation is entirely row-level security --
    -- `ALTER TABLE ... DISABLE ROW LEVEL SECURITY` and `DROP POLICY`.
    -- `console_rt` is on the write ratchet because it is the runtime login
    -- principal; letting that entry also authorise it to OWN `employees` would
    -- hand the runtime the ability to switch its own RLS off. So the owner is
    -- pinned by `expected_owners`, which the census's ratchet cannot widen, over
    -- the same reachable set the census examined.
    --
    -- canonical-writer-ownership: BEGIN owner pin. This is the ONLY place the
    -- owner catalog may be read, and `topology_script_names_the_roles_that_lose_dml`
    -- counts `relowner` occurrences in the whole block against this region to
    -- keep it so. Read anywhere the census can reach — including one statement
    -- above the census markers, joined in afterwards — ownership becomes
    -- subject to the census's ratchet, which excludes a candidate by NAME
    -- before it asks the privilege question.
    SELECT string_agg(
             relation.relname || ':' || pg_get_userbyid(relation.relowner),
             ', ' ORDER BY relation.relname
           )
    INTO offending
    FROM unnest(examined) AS examined_relation(oid)
    JOIN pg_class relation ON relation.oid = examined_relation.oid
    WHERE pg_get_userbyid(relation.relowner) <> ALL (expected_owners);
    IF offending IS NOT NULL THEN
        RAISE EXCEPTION 'topology.canonical_table_owner_failed: unexpected owner of a canonical table: %', offending;
    END IF;
    -- canonical-writer-ownership: END owner pin

    IF EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'canonical_api')
       AND NOT EXISTS (
        SELECT 1 FROM pg_namespace
        WHERE nspname = 'canonical_api'
          AND nspowner = (SELECT oid FROM pg_roles WHERE rolname = 'console_app')
    ) THEN
        RAISE EXCEPTION 'topology.canonical_api_owner_failed';
    END IF;
END
$canonical$;

SELECT 'DROP ROLE console_legacy_conversion_admin'
WHERE :'legacy_reassign' = '1' \gexec
COMMIT;
SQL

# Role defaults affect only new sessions. Capture every extant serving-role
# backend after commit, synchronously terminate each one with a positive timeout,
# and prove that exact captured set is absent before returning.
serving_backend_pid_output="$(psql "${admin_psql_args[@]}" -Atqc \
  "SELECT pid FROM pg_stat_activity WHERE usename IN ('console_rt','console_leave_cmd','console_ontology_cmd','console_platform_force_cmd') AND pid <> pg_backend_pid() ORDER BY pid")"
if [[ -n "${serving_backend_pid_output}" ]]; then
  while IFS= read -r pid; do
    terminated="$(psql "${admin_psql_args[@]}" -Atqc \
      "SELECT pg_terminate_backend(${pid}, 5000)")"
    if [[ "${terminated}" != "t" ]]; then
      echo "topology.serving_backend_termination_failed: ${pid}" >&2
      exit 1
    fi
  done <<<"${serving_backend_pid_output}"
  captured_pid_csv="${serving_backend_pid_output//$'\n'/,}"
  remaining="$(psql "${admin_psql_args[@]}" -Atqc \
    "SELECT count(*) FROM pg_stat_activity WHERE pid = ANY (ARRAY[${captured_pid_csv}]::integer[])")"
  if [[ "${remaining}" != "0" ]]; then
    echo "topology.serving_backend_drain_barrier_failed" >&2
    exit 1
  fi
fi
verify_serving_login() {
  local role="$1"
  local password="$2"
  local actual
  actual="$(PGPASSWORD="${password}" psql \
    --host "${POSTGRES_HOST}" --port "${POSTGRES_PORT}" \
    --username "${role}" --dbname "${POSTGRES_DB}" \
    --set ON_ERROR_STOP=1 -At -F '|' -c \
    "SELECT session_user,current_user,current_setting('statement_timeout'),current_setting('idle_in_transaction_session_timeout'),current_setting('transaction_timeout')")"
  if [[ "${actual}" != "${role}|${role}|30s|30s|45s" ]]; then
    echo "topology.runtime_default_effective_readback_failed: ${role}" >&2
    exit 1
  fi
}
verify_serving_login console_rt "${CONSOLE_RT_POSTGRES_PASSWORD}"
verify_serving_login console_leave_cmd "${CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD}"
verify_serving_login console_ontology_cmd "${CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD}"
verify_serving_login console_platform_force_cmd "${CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD}"

echo "topology: eight application roles reconciled and verified" >&2
