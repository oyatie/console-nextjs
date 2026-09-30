#!/usr/bin/env bash
# Disposable PostgreSQL for a single `cargo test` run, mirroring
# tools/buck/test_needs_postgres.sh without going through Buck.
#
# --rm is not optional: 707 orphaned volumes once filled this VM and took dev
# Postgres down with it. The trap removes the container on every exit path.
set -euo pipefail
repo_root="${1:?repo root}"; shift

# Refuse a command line that already carries a database credential. The guard is
# its own file so scripts/check-test-credentials.mjs can exec it directly and
# prove it in both directions -- this harness therefore has no bypass at all.
# shellcheck source=no-credential-in-argv.sh
source "$(dirname "${BASH_SOURCE[0]}")/no-credential-in-argv.sh" "$@"

image="postgres:18.4@sha256:65f70a152846cf504dff86e807007e9aeac98c3aeb7b62541b2c55ab9d264e56"
name="console-conformance-$$"
db="console_conformance_$$"

envf=""   # referenced by the EXIT trap, which can fire before it is assigned
# Hygiene is measured on THIS run's own resources, never on a global count.
# A global before/after count is confounded the moment another agent runs a
# container concurrently -- observed reading "34 before -> 35 after" while the
# only new volume belonged to a peer. A leak detector that reports someone
# else's activity as your leak is worse than none, because it trains you to
# ignore it.
cleanup() {
  # -v is not optional: `docker rm -f` alone strands the anonymous volume, and
  # 707 stranded volumes once filled this VM and took dev Postgres down with it.
  docker rm -fv "$name" >/dev/null 2>&1 || true
  [ -n "$envf" ] && rm -f "$envf" 2>/dev/null
  local leaked
  leaked="$(docker ps -aq --filter "name=^${name}$" 2>/dev/null | wc -l | tr -d ' ')"
  if [ "$leaked" != "0" ]; then
    echo "LEAK: container ${name} survived cleanup" >&2
  else
    echo "clean: ${name} removed with its volume"
  fi
}
trap cleanup EXIT

# Not `tr </dev/urandom | head -c 64`: head closes the pipe, tr dies on SIGPIPE,
# and `set -o pipefail` turns that into an exit before anything is created.
# Not BSD `od -An` either -- that spelling is GNU-only and fails on macOS.
pw() { openssl rand -hex 32; }
admin="$(pw)"; app="$(pw)"; rt="$(pw)"; leave="$(pw)"; ont="$(pw)"; force="$(pw)"

umask 077
envf="$(mktemp "${TMPDIR:-/tmp}/conformance-pg.XXXXXX")"; chmod 600 "$envf"
{
  printf 'POSTGRES_DB=%s\nPOSTGRES_USER=console_buck_admin\nPOSTGRES_PASSWORD=%s\n' "$db" "$admin"
  printf 'POSTGRES_HOST=127.0.0.1\nPOSTGRES_PORT=5432\nPOSTGRES_ADMIN_USER=console_buck_admin\nPOSTGRES_ADMIN_PASSWORD=%s\n' "$admin"
  printf 'CONSOLE_APP_POSTGRES_PASSWORD=%s\nCONSOLE_RT_POSTGRES_PASSWORD=%s\n' "$app" "$rt"
  printf 'CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD=%s\nCONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD=%s\n' "$leave" "$ont" "$force"
} >"$envf"

docker run -d --rm --name "$name" -p 127.0.0.1::5432 --env-file "$envf" "$image" >/dev/null
docker cp "$repo_root/ops/postgres-reconcile-topology.sh" "$name:/topology.sh" >/dev/null
docker cp "$envf" "$name:/topology.env" >/dev/null

for i in $(seq 1 40); do
  if [ "$(docker exec "$name" cat /proc/1/comm 2>/dev/null || true)" = "postgres" ] \
     && docker exec "$name" pg_isready -h 127.0.0.1 -U console_buck_admin -d "$db" >/dev/null 2>&1; then break; fi
  [ "$i" = 40 ] && { echo "postgres never became healthy"; exit 1; }
  sleep 1
done
docker exec "$name" sh -ceu 'set -a; . /topology.env; exec bash /topology.sh' >/dev/null

# That reconcile ran before any migration, so its canonical writer-ownership
# census had zero canonical tables to look at. Re-run it on a migrated probe
# database with the zero-table guard armed; "$db" itself is left untouched.
bash "$repo_root/backend/ci/gates/writer-ownership/canonical-enforce.sh" \
  "$repo_root" "$name" "canonical_probe_$$"

port="$(docker port "$name" 5432/tcp)"; port="${port##*:}"
export DATABASE_URL="postgres://console_buck_admin:${admin}@127.0.0.1:${port}/${db}?options%5Bconsole.sqlx_test_bootstrap%5D=buck-sqlx-superuser-v1"
echo "postgres ready on ${port}"
cd "$repo_root/backend"
"$@"
