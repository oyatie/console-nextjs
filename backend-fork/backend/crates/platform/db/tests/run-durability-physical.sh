#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../../.." && pwd)
pg_bin=${PG18_BIN:-/opt/homebrew/opt/postgresql@18/bin}
if [[ ! -x "$pg_bin/postgres" || $("$pg_bin/postgres" --version) != *'PostgreSQL) 18.'* ]]; then
    echo 'PG18_BIN must name PostgreSQL 18 binaries' >&2
    exit 2
fi

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/v1-durability.XXXXXX")
cleanup() {
    "$pg_bin/pg_ctl" -D "$work_dir/standby" -m immediate -w stop >/dev/null 2>&1 || true
    "$pg_bin/pg_ctl" -D "$work_dir/primary" -m immediate -w stop >/dev/null 2>&1 || true
    rm -rf -- "$work_dir"
}
trap cleanup EXIT

read -r primary_port standby_port < <(python3 - <<'PY'
import socket
with socket.socket() as primary, socket.socket() as standby:
    primary.bind(('127.0.0.1', 0))
    standby.bind(('127.0.0.1', 0))
    print(primary.getsockname()[1], standby.getsockname()[1])
PY
)

printf '%s\n' 'disposable_probe_only' > "$work_dir/password"
"$pg_bin/initdb" -D "$work_dir/primary" -U postgres \
    --auth-local=trust --auth-host=scram-sha-256 \
    --pwfile="$work_dir/password" --no-instructions >/dev/null
cat >> "$work_dir/primary/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $primary_port
unix_socket_directories = '$work_dir'
wal_level = replica
wal_log_hints = on
wal_writer_delay = '10s'
max_wal_senders = 5
synchronous_commit = remote_apply
synchronous_standby_names = ''
EOF
"$pg_bin/pg_ctl" -D "$work_dir/primary" -l "$work_dir/primary.log" -w start >/dev/null
export PGPASSWORD=disposable_probe_only
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c "CREATE ROLE replica LOGIN REPLICATION PASSWORD 'disposable_probe_only'" >/dev/null
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c 'CREATE ROLE console_rt NOLOGIN' >/dev/null
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c "CREATE ROLE durability_observer LOGIN PASSWORD 'disposable_probe_only'" >/dev/null
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c 'GRANT pg_monitor TO durability_observer' >/dev/null
"$pg_bin/pg_basebackup" -D "$work_dir/standby" -h 127.0.0.1 \
    -p "$primary_port" -U replica -X stream -R >/dev/null
cat >> "$work_dir/standby/postgresql.auto.conf" <<EOF
primary_conninfo = 'host=127.0.0.1 port=$primary_port user=replica password=disposable_probe_only application_name=v1_standby'
EOF
cat >> "$work_dir/standby/postgresql.conf" <<EOF
listen_addresses = '127.0.0.1'
port = $standby_port
unix_socket_directories = '$work_dir'
hot_standby = on
EOF
"$pg_bin/pg_ctl" -D "$work_dir/standby" -l "$work_dir/standby.log" -w start >/dev/null
streaming=0
for _ in {1..50}; do
    if [[ $("$pg_bin/psql" -X -A -t -h 127.0.0.1 -p "$primary_port" -U postgres \
        -c "SELECT count(*) FROM pg_stat_replication WHERE application_name = 'v1_standby' AND state = 'streaming'") == 1 ]]; then
        streaming=1
        break
    fi
    sleep 0.1
done
if [[ $streaming != 1 ]]; then
    echo 'physical standby did not reach streaming state' >&2
    exit 1
fi
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c "ALTER SYSTEM SET synchronous_standby_names = 'FIRST 1 (v1_standby)'" >/dev/null
"$pg_bin/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$primary_port" -U postgres \
    -c 'SELECT pg_reload_conf()' >/dev/null

export V1_PRIMARY_DSN="postgres://postgres:disposable_probe_only@127.0.0.1:$primary_port/postgres"
export V1_STANDBY_DSN="postgres://postgres:disposable_probe_only@127.0.0.1:$standby_port/postgres"
export V1_OBSERVER_PRIMARY_DSN="postgres://durability_observer:disposable_probe_only@127.0.0.1:$primary_port/postgres"
export V1_OBSERVER_STANDBY_DSN="postgres://durability_observer:disposable_probe_only@127.0.0.1:$standby_port/postgres"
export V1_PRIMARY_PORT="$primary_port"
export V1_STANDBY_PORT="$standby_port"
export V1_PRIMARY_DATA_DIR="$work_dir/primary"
export SQLX_OFFLINE=true
cd "$repo_root/backend"
cargo test -p console-platform-db --features test-physical-replication \
    --test durability_physical -- --exact confirms_only_a_fenced_physical_prefix --nocapture
