#!/usr/bin/env bash
# Canonical writer-ownership enforcement against a MIGRATED database.
#
# The reconcile's canonical census can only see the tables that exist when it
# runs. Every automated path -- tools/ci/cargo_needs_postgres.sh,
# tools/lanes/pgtest.sh, .github/workflows/image-release.yml -- runs the
# reconcile BEFORE migrations, so the census saw zero canonical tables and
# returned NULL unconditionally. That is a structural no-op, not a pass.
#
# WHICH OF THOSE THREE THIS SCRIPT IS ACTUALLY WIRED INTO, because listing three
# paths above and covering two is the same false-claim-about-a-control this
# script exists to remove:
#     tools/ci/cargo_needs_postgres.sh   COVERED
#     tools/lanes/pgtest.sh              COVERED
#     .github/workflows/image-release.yml  COVERED
# image-release.yml invokes this script against a disposable probe database
# inside the release postgres container after the published image has applied
# migrations (console-soe). The pre-migration reconcile step remains a topology
# smoke only; post-migration canonical enforcement is this script.
#
# This is the post-migration half: a disposable probe database inside an
# already-running container, migrated with the real migration set, then handed
# to the reconcile again with CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES=1 so
# that examining zero canonical tables FAILS instead of passing silently.
#
# The caller must already have copied ops/postgres-reconcile-topology.sh to
# /topology.sh and its environment to /topology.env in the container.
#
# usage: canonical-enforce.sh <repo_root> <container> <probe_db>
#                             [--skip-migrations | <extra_sql_path>]
#
#   --skip-migrations   deliberately leave the probe database bare, to prove the
#                       zero-table guard fires. Used only by the census test.
#   <extra_sql_path>    SQL applied as the cluster admin AFTER migrations and
#                       BEFORE enforcement. It can only add grants; the
#                       enforcement still runs. Used by the census test to plant
#                       a rogue writer.
#
# CANONICAL_REQUIRE_TABLES (env, default 1) selects
# CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES for the enforcement run. It defaults
# to 1 because that is the point of this script: every automated path used to
# run the reconcile before migrations, so the census saw nothing and passed.
# The census test sets it to 0 for exactly two mutations, whose controls are the
# only thing looking in the UNARMED configuration -- see `Mutation::armed` in
# tests/census_executes_against_postgres.rs.
set -euo pipefail

repo_root="${1:?repo root}"
container="${2:?container name}"
probe_db="${3:?probe database name}"
extra="${4:-}"

case "${probe_db}" in
  [a-z_][a-z0-9_]*) ;;
  *) echo "canonical-enforce: probe database name must be a bare lowercase identifier" >&2; exit 2 ;;
esac

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

docker cp "${repo_root}/backend/crates/platform/db/migrations" \
  "${container}:/canonical-migrations" >/dev/null
docker cp "${here}/canonical-enforce-in-container.sh" \
  "${container}:/canonical-enforce-in-container.sh" >/dev/null

skip_migrations=0
extra_in_container=""
if [[ "${extra}" == "--skip-migrations" ]]; then
  skip_migrations=1
elif [[ -n "${extra}" ]]; then
  docker cp "${extra}" "${container}:/canonical-extra.sql" >/dev/null
  extra_in_container=/canonical-extra.sql
fi

require_tables="${CANONICAL_REQUIRE_TABLES:-1}"
case "${require_tables}" in
  0|1) ;;
  *) echo "canonical-enforce: CANONICAL_REQUIRE_TABLES must be 0 or 1" >&2; exit 2 ;;
esac

docker exec \
  -e "CANONICAL_PROBE_DB=${probe_db}" \
  -e "CANONICAL_SKIP_MIGRATIONS=${skip_migrations}" \
  -e "CANONICAL_EXTRA_SQL=${extra_in_container}" \
  -e "CANONICAL_REQUIRE_TABLES=${require_tables}" \
  "${container}" bash /canonical-enforce-in-container.sh
