#!/usr/bin/env python3
"""Verify install lineages on a disposable, socket-only PostgreSQL cluster."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode

from acceptance import cargo_counts, inspect_fork
from schema_baseline import BASELINE_PATH, CATALOGS, baseline_sql, canonical_dump, catalog_rows, historical_script

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--postgres-bin", required=True, type=Path)
    parser.add_argument("--app", type=Path, default=ROOT / ".artifacts/cargo-target/debug/console-app")
    parser.add_argument("--legacy-app", type=Path, help="Frozen pre-transition binary, for actual old-runner refusal proof")
    parser.add_argument("--baseline-app", type=Path, help="Preserved baseline-aware pre-suffix binary, for installed-baseline upgrade proof")
    parser.add_argument("--account-lock-baseline-app", type=Path, help="Preserved accepted0226 binary for actual0227 interruption and retry proof")
    parser.add_argument("--identity-baseline-app", type=Path, help="Preserved accepted0227 binary for actual0228 interruption and retry proof")
    parser.add_argument("--removal-baseline-app", type=Path, help="Preserved accepted0228 binary for actual0229 interruption and retry proof")
    parser.add_argument("--auth-state-baseline-app", type=Path, help="Preserved accepted0229 binary for actual0230/0231 interruption and retry proof")
    parser.add_argument("--migration-build-probe", action="store_true", help="Run fixed-source incremental SQLx build dependency proof in the disposable cluster")
    parser.add_argument("--generate-baseline", action="store_true", help="Generate schema from isolated historical replay; does not run acceptance")
    parser.add_argument("--cargo-tests", action="store_true", help="Also run inherited Rust library tests with isolated SQLx credentials")
    parser.add_argument("--cargo-integration", action="append", default=[], metavar="PACKAGE:TARGET", help="Run a named integration target with disposable SQLx credentials")
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    pg = args.postgres_bin.resolve(strict=True)
    evidence = ROOT / ".artifacts" / datetime.now(timezone.utc).strftime("empty-install-%Y%m%dT%H%M%S.%fZ")
    evidence.mkdir(parents=True)
    report = {"scope": "single-site install/upgrade regression; not two-site durability or release proof", "status": "unreached", "commands": [], "checks": [], "app_sha256": hashlib.sha256(app.read_bytes()).hexdigest()}
    env = {k: v for k, v in os.environ.items() if k in {"HOME", "TMPDIR", "LANG", "LC_ALL"}}
    env["PATH"] = os.pathsep.join((str(pg), str(Path(env["HOME"]) / ".cargo/bin"), "/usr/bin", "/bin", "/usr/sbin", "/sbin", "/opt/homebrew/bin"))
    passwords = {key: secrets.token_hex(32) for key in (
        "POSTGRES_ADMIN_PASSWORD", "CONSOLE_APP_POSTGRES_PASSWORD", "CONSOLE_RT_POSTGRES_PASSWORD",
        "CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD", "CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD",
        "CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD",
    )}

    # All disposable connections authenticate, including role-separation regression tests.
    env["PGPASSWORD"] = passwords["POSTGRES_ADMIN_PASSWORD"]

    def run(name, command, extra=None, capture=False, timeout=120, cwd=ROOT, allow_failure=False):
        result = subprocess.run(command, cwd=cwd, env={**env, **(extra or {})}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout)
        output = result.stdout
        for value in passwords.values():
            output = output.replace(value.encode(), b"[REDACTED]")
        (evidence / f"{name}.log").write_bytes(output)
        report["commands"].append({"name": name, "command": command, "exit_code": result.returncode, "output_sha256": hashlib.sha256(output).hexdigest()})
        if result.returncode and not allow_failure:
            raise RuntimeError(f"{name} failed ({result.returncode}); see {evidence / (name + '.log')}")
        return output.decode() if capture else result.returncode

    def check(name, condition):
        report["checks"].append({"name": name, "status": "passed" if condition else "failed"})
        if not condition:
            raise AssertionError(name)

    try:
        report["source_closure"] = inspect_fork(ROOT)
        check("reviewed-migration-source-custody", report["source_closure"]["status"] == "passed")
        lineages = report["source_closure"]["lineages"]
        fresh_versions = lineages["fresh"]
        historical_versions = lineages["historical"]
        # SQLx uses SHA-384 of the exact applied SQL, independent of source SHA-256.
        migration_checksums = {0: hashlib.sha384((ROOT / BASELINE_PATH).read_bytes()).hexdigest()}
        migration_folder = ROOT / "backend-fork/backend/crates/platform/db/migrations"
        for file in migration_folder.iterdir():
            migration_checksums[int(file.name.split("_", 1)[0])] = hashlib.sha384(file.read_bytes()).hexdigest()

        def exact_ledger(name, ledger, expected):
            check(name + "-versions", [row["version"] for row in ledger] == expected)
            check(name + "-checksums", all(row["success"] and row["checksum"] == "\\x" + migration_checksums[row["version"]] for row in ledger))

        with tempfile.TemporaryDirectory(prefix="frontend-v1-pg-", dir="/private/tmp") as temp:
            directory = Path(temp)
            socket = directory / "socket"
            socket.mkdir(mode=0o700)
            data = directory / "data"
            ctl = [str(pg / "pg_ctl"), "-D", str(data)]
            admin = "console_buck_admin"
            password_file = directory / "admin-password"
            password_file.write_text(passwords["POSTGRES_ADMIN_PASSWORD"] + "\n")
            password_file.chmod(0o600)
            run("initdb", [str(pg / "initdb"), "-D", str(data), "--username=" + admin, "--auth-local=scram-sha-256", "--pwfile=" + str(password_file), "--auth-host=reject", "--no-locale", "--encoding=UTF8"])
            password_file.unlink()

            def psql(name, db, sql, user=admin, allow_failure=False):
                file = directory / (name + ".sql")
                file.write_text(sql)
                return run(name, [str(pg / "psql"), "-X", "-qAt", "-h", str(socket), "-U", user, "-d", db, "-v", "ON_ERROR_STOP=1", "-f", str(file)], {"PGPASSWORD": passwords["CONSOLE_APP_POSTGRES_PASSWORD"] if user == "console_app" else passwords["CONSOLE_RT_POSTGRES_PASSWORD"] if user == "console_rt" else passwords["POSTGRES_ADMIN_PASSWORD"]}, capture=True, allow_failure=allow_failure)

            def wait_for_pid(db, predicate):
                # Poll in separate short transactions. A single DO/pg_sleep
                # watcher holds a virtual transaction and blocks concurrent
                # index builds in the migrator it is trying to observe.
                query = ("SELECT pid FROM pg_stat_activity WHERE datname=current_database() "
                         "AND usename='console_app' AND state='active' AND " + predicate + " LIMIT 1")
                command = [str(pg / "psql"), "-X", "-qAt", "-h", str(socket),
                           "-U", admin, "-d", db, "-v", "ON_ERROR_STOP=1", "-c", query]
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    result = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, timeout=2)
                    if result.returncode:
                        raise RuntimeError("migration boundary poll failed")
                    if result.stdout.strip():
                        return int(result.stdout.strip())
                    time.sleep(0.01)
                raise AssertionError("target boundary never reached")

            def wait_for_lock(db):
                query = "SELECT pid FROM pg_locks WHERE locktype='advisory' AND objid=901011 AND granted LIMIT 1"
                command = [str(pg / "psql"), "-X", "-qAt", "-h", str(socket),
                           "-U", admin, "-d", db, "-v", "ON_ERROR_STOP=1", "-c", query]
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    result = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                                            stderr=subprocess.PIPE, timeout=2)
                    if result.returncode:
                        raise RuntimeError("Apalis lock poll failed")
                    if result.stdout.strip():
                        return
                    time.sleep(0.01)
                raise AssertionError("test lock not acquired")

            def create(db):
                run("create-" + db, [str(pg / "createdb"), "-h", str(socket), "-U", admin, db])
                run("topology-" + db, ["bash", str(ROOT / "backend-fork/ops/postgres-reconcile-topology.sh")], {
                    **passwords, "POSTGRES_HOST": str(socket), "POSTGRES_DB": db,
                    "POSTGRES_ADMIN_USER": admin, "POSTGRES_LOCAL_SOCKET_DIR": str(socket),
                })

            def migration_env(db):
                url = "postgresql://console_app:" + passwords["CONSOLE_APP_POSTGRES_PASSWORD"] + "@localhost/" + db + "?" + urlencode({"host": str(socket)})
                return {"CONSOLE_APP_ROLE": "migrate", "DATABASE_URL": url}

            def migrate(name, db, binary=app, allow_failure=False):
                return run(name, [str(binary)], migration_env(db), allow_failure=allow_failure)

            def rows(name, db):
                tables = psql(name + "-tables", db, "SELECT schemaname || '.' || tablename FROM pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema','apalis') ORDER BY 1").splitlines()
                query = "\n".join(f"SELECT jsonb_build_object('table','{table}','rows',coalesce(jsonb_agg(t), '[]'::jsonb)) FROM {table} t;" for table in tables)
                return {item["table"].removeprefix("public."): sorted(item["rows"], key=lambda row: row["version"] if item["table"] == "public._sqlx_migrations" else json.dumps(row, sort_keys=True)) for item in (json.loads(line) for line in psql(name, db, query).splitlines())}

            def schema(name, db):
                return run(name, [str(pg / "pg_dump"), "-h", str(socket), "-U", admin, "-d", db, "--schema-only", "--exclude-schema=apalis", "--exclude-table=public._sqlx_migrations"], capture=True)

            def interrupt_apply(db, after_commit=False):
                create(db)
                with ThreadPoolExecutor(max_workers=2) as workers:
                    locker = None
                    if after_commit:
                        # Hold the Apalis owner lock from an idle session. A
                        # transaction kept open by pg_sleep would also block
                        # CREATE INDEX CONCURRENTLY before the schema COMMIT.
                        locker = workers.submit(psql, db + "-lock", db,
                            "SELECT pg_advisory_lock(901011);\n\\! sleep 12\nSELECT pg_advisory_unlock(901011);")
                        wait_for_lock(db)
                    running = workers.submit(migrate, db + "-interrupted", db, app, True)
                    condition = "query LIKE '-- Generated from the frozen historical schema%'"
                    if after_commit:
                        condition = "pid IN (SELECT pid FROM pg_locks WHERE locktype='advisory' AND objid=901011 AND NOT granted)"
                    function = "pg_terminate_backend" if after_commit else "pg_cancel_backend"
                    target = wait_for_pid(db, condition)
                    check(db + "-interrupt", psql(db + "-interrupt", db,
                          f"SELECT {function}({target})").strip() == "t")
                    check(db + "-process-failed", running.result() != 0)
                    if locker:
                        locker.result()
                interrupted = rows(db + "-interrupted-rows", db)
                if after_commit:
                    check(db + "-commit-survived", [row["version"] for row in interrupted["_sqlx_migrations"]] == fresh_versions)
                    exact_ledger(db + "-committed-ledger", interrupted["_sqlx_migrations"], fresh_versions)
                else:
                    check(db + "-transaction-rolled-back", interrupted == {"_sqlx_migrations": []})
                migrate(db + "-retry", db)
                retried = rows(db + "-retry-rows", db)
                check(db + "-retry-empty", all(not data for table, data in retried.items() if table not in (*CATALOGS, "_sqlx_migrations")))
                if after_commit:
                    check(db + "-no-reapplication", interrupted["_sqlx_migrations"] == retried["_sqlx_migrations"])


            def interrupt_credential_suffix():
                # Test-only installed-baseline upgrade interrupted at the actual
                # credential trigger's conflicting relation lock, before commit.
                db = "credential_suffix_interrupt"
                create(db)
                migrate(db + "-baseline", db, args.baseline_app.resolve(strict=True))
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000003','suffix-client','Suffix upgrade fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000004','Existing key owner','12340000-0000-0000-0000-000000000003',ARRAY['MEMBER']); INSERT INTO auth_webauthn_credentials(user_id,credential_id,passkey_json,org_id) VALUES ('12340000-0000-0000-0000-000000000004','unknown-legacy-codec','{}','12340000-0000-0000-0000-000000000003');")
                before = rows(db + "-before", db)
                before_schema = canonical_dump(schema(db + "-before-schema", db))
                with ThreadPoolExecutor(max_workers=2) as workers:
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; LOCK TABLE public.auth_webauthn_credentials IN SHARE MODE; SELECT pg_sleep(12); COMMIT;")
                    psql(db + "-lock-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_locks WHERE relation='public.auth_webauthn_credentials'::regclass
                            AND mode='ShareLock' AND granted) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'credential lock not acquired'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-cancel", db, """DO $$ DECLARE target integer; BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        SELECT a.pid INTO target FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid
                        WHERE a.datname=current_database() AND a.usename='console_app'
                          AND l.relation='public.auth_webauthn_credentials'::regclass
                          AND l.mode='ShareRowExclusiveLock' AND NOT l.granted
                          AND cardinality(pg_blocking_pids(a.pid))>0 LIMIT 1;
                        IF target IS NOT NULL THEN
                            IF NOT pg_cancel_backend(target) THEN RAISE EXCEPTION 'cancel failed'; END IF;
                            RETURN;
                        END IF; PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual suffix lock boundary not reached'; END $$;""")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-no-partial-function-or-trigger", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                check(db + "-all-existing-rows-preserved", all(after[t] == values for t, values in before.items() if t != "_sqlx_migrations"))
                check(db + "-existing-ledger-metadata-preserved", after["_sqlx_migrations"][:len(before["_sqlx_migrations"])] == before["_sqlx_migrations"])
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-trigger-present", psql(db + "-trigger", db, "SELECT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid='public.auth_webauthn_credentials'::regclass AND tgname='auth_webauthn_credential_update_guard' AND tgenabled='O')").strip() == "t")
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)


            def interrupt_account_lock_suffix():
                # Use the accepted pre0227 installer, never a hand-made SQLx ledger.
                db = "account_lock_suffix_interrupt"
                previous = args.account_lock_baseline_app.resolve(strict=True)
                report["account_lock_baseline_app_sha256"] = hashlib.sha256(previous.read_bytes()).hexdigest()
                check(db + "-accepted-installer", report["account_lock_baseline_app_sha256"] == "8e86b652ebb9e8531af3a221f06c47c7d1306d1a380fdb34afe00a7559816e76")
                create(db)
                migrate(db + "-baseline", db, previous)
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000013','lock-suffix-client','Lock suffix fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000014','Preserved account','12340000-0000-0000-0000-000000000013',ARRAY['MEMBER']);")
                before = rows(db + "-before", db)
                exact_ledger(db + "-prior-ledger", before["_sqlx_migrations"], [0, 226])
                before_schema = canonical_dump(schema(db + "-before-schema", db))
                with ThreadPoolExecutor(max_workers=2) as workers:
                    # PostgreSQL's own CREATE OR REPLACE FUNCTION catalog write
                    # is the boundary. Restrict cancellation to the actual0227 SQL.
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; LOCK TABLE pg_catalog.pg_proc IN SHARE MODE; SELECT pg_sleep(12); COMMIT;")
                    psql(db + "-lock-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_locks WHERE relation='pg_catalog.pg_proc'::regclass
                            AND mode='ShareLock' AND granted) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'catalog lock not acquired'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-cancel", db, """DO $$ DECLARE target integer; BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        SELECT a.pid INTO target FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid
                        WHERE a.datname=current_database() AND a.usename='console_app'
                          AND a.query LIKE '-- Coordinated authentication Account locks%'
                          AND l.relation='pg_catalog.pg_proc'::regclass
                          AND l.mode='RowExclusiveLock' AND NOT l.granted
                          AND cardinality(pg_blocking_pids(a.pid))>0 LIMIT 1;
                        IF target IS NOT NULL THEN
                            IF NOT pg_cancel_backend(target) THEN RAISE EXCEPTION 'cancel failed'; END IF;
                            RETURN;
                        END IF; PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual0227catalog boundary not reached'; END $$;""")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-function-acl-schema-rolled-back", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                check(db + "-all-existing-rows-preserved", all(after[t] == values for t, values in before.items() if t != "_sqlx_migrations"))
                check(db + "-existing-ledger-metadata-preserved", after["_sqlx_migrations"][:len(before["_sqlx_migrations"])] == before["_sqlx_migrations"])
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-guard-private", psql(db + "-guard-acl", db, "SELECT to_regprocedure('platform_lock_organization_accounts_for_removal(uuid)') IS NOT NULL AND NOT has_function_privilege('console_rt','platform_lock_organization_accounts_for_removal(uuid)','EXECUTE') AND NOT has_function_privilege('console_platform_force_cmd','platform_lock_organization_accounts_for_removal(uuid)','EXECUTE')").strip() == "t")
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)

            def interrupt_identity_suffix():
                db = "identity_suffix_interrupt"
                previous = args.identity_baseline_app.resolve(strict=True)
                digest = hashlib.sha256(previous.read_bytes()).hexdigest()
                report["identity_baseline_app_sha256"] = digest
                check(db + "-accepted-installer", digest == "610a0ef34336e4c356936a3f46017d091a5365189abcdceac39166f950145c0d")
                create(db)
                migrate(db + "-baseline", db, previous)
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000023','identity-suffix-client','Identity suffix fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000024','Preserved identity','12340000-0000-0000-0000-000000000023',ARRAY['MEMBER']);")
                before = rows(db + "-before", db)
                exact_ledger(db + "-prior-ledger", before["_sqlx_migrations"], [0, 226, 227])
                before_schema = canonical_dump(schema(db + "-before-schema", db))
                with ThreadPoolExecutor(max_workers=2) as workers:
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; LOCK TABLE public.users IN ROW EXCLUSIVE MODE; SELECT pg_sleep(12); COMMIT;")
                    psql(db + "-lock-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_locks WHERE relation='public.users'::regclass
                            AND mode='RowExclusiveLock' AND granted) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'Account relation lock not acquired'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-cancel", db, """DO $$ DECLARE target integer; BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        SELECT a.pid INTO target FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid
                        WHERE a.datname=current_database() AND a.usename='console_app'
                          AND a.query LIKE '-- Account UUID allocation and retirement%'
                          AND l.relation='public.users'::regclass
                          AND l.mode='ShareRowExclusiveLock' AND NOT l.granted
                          AND cardinality(pg_blocking_pids(a.pid))>0 LIMIT 1;
                        IF target IS NOT NULL THEN
                            IF NOT pg_cancel_backend(target) THEN RAISE EXCEPTION 'cancel failed'; END IF;
                            RETURN;
                        END IF; PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual0228Account boundary not reached'; END $$;""")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-schema-acl-rolled-back", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                check(db + "-all-existing-rows-preserved", all(after[t] == values for t, values in before.items() if t != "_sqlx_migrations"))
                check(db + "-prior-ledger-exact", after["_sqlx_migrations"][:len(before["_sqlx_migrations"])] == before["_sqlx_migrations"])
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-all-identities-reserved", psql(db + "-allocations", db, "SELECT (SELECT array_agg(id ORDER BY id) FROM users)=(SELECT array_agg(account_id ORDER BY account_id) FROM auth_security.account_id_reservations WHERE NOT retired)").strip() == "t")
                check(db + "-private", psql(db + "-private-acl", db, "SELECT NOT has_schema_privilege('console_rt','auth_security','USAGE') AND NOT has_table_privilege('console_rt','auth_security.account_id_reservations','SELECT,INSERT,UPDATE,DELETE,TRUNCATE')").strip() == "t")
                post_schema = canonical_dump(schema(db + "-after-schema", db))
                run(db + "-topology-reapply", ["bash", str(ROOT / "backend-fork/ops/postgres-reconcile-topology.sh")], {
                    **passwords, "POSTGRES_HOST": str(socket), "POSTGRES_DB": db,
                    "POSTGRES_ADMIN_USER": admin, "POSTGRES_LOCAL_SOCKET_DIR": str(socket),
                    "CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES": "1",
                })
                check(db + "-topology-schema-acl-preserved", canonical_dump(schema(db + "-reapplied-schema", db)) == post_schema)
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)

            def interrupt_removal_suffix():
                db = "removal_suffix_interrupt"
                previous = args.removal_baseline_app.resolve(strict=True)
                digest = hashlib.sha256(previous.read_bytes()).hexdigest()
                report["removal_baseline_app_sha256"] = digest
                check(db + "-accepted-installer", digest == "291dde4b70d69e0de48d8ea830cbaf0128d46daa23fa541f1fe7344bfcc6be16")
                create(db)
                migrate(db + "-baseline", db, previous)
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000023','identity-suffix-client','Identity suffix fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000024','Preserved identity','12340000-0000-0000-0000-000000000023',ARRAY['MEMBER']);")
                psql(db + "-key-fixture", db, "INSERT INTO auth_webauthn_credentials(id,user_id,org_id,credential_id,passkey_json) VALUES ('12340000-0000-0000-0000-000000000025','12340000-0000-0000-0000-000000000024','12340000-0000-0000-0000-000000000023','preserved-key','{}');")
                before = rows(db + "-before", db)
                exact_ledger(db + "-prior-ledger", before["_sqlx_migrations"], [0, 226, 227, 228])
                before_schema = canonical_dump(schema(db + "-before-schema", db))
                with ThreadPoolExecutor(max_workers=2) as workers:
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; LOCK TABLE public.auth_webauthn_credentials IN ROW EXCLUSIVE MODE; SELECT pg_sleep(12); COMMIT;")
                    psql(db + "-lock-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_locks WHERE relation='public.auth_webauthn_credentials'::regclass
                            AND mode='RowExclusiveLock' AND granted) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'credential relation lock not acquired'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-cancel", db, """DO $$ DECLARE target integer; BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        SELECT a.pid INTO target FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid
                        WHERE a.datname=current_database() AND a.usename='console_app'
                          AND a.query LIKE '-- Credential removal evidence%'
                          AND l.relation='public.auth_webauthn_credentials'::regclass
                          AND l.mode='ShareRowExclusiveLock' AND NOT l.granted
                          AND cardinality(pg_blocking_pids(a.pid))>0 LIMIT 1;
                        IF target IS NOT NULL THEN
                            IF NOT pg_cancel_backend(target) THEN RAISE EXCEPTION 'cancel failed'; END IF;
                            RETURN;
                        END IF; PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual0229credential boundary not reached'; END $$;""")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-schema-acl-rolled-back", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                check(db + "-all-existing-rows-preserved", all(after[t] == values for t, values in before.items() if t != "_sqlx_migrations"))
                check(db + "-prior-ledger-exact", after["_sqlx_migrations"][:len(before["_sqlx_migrations"])] == before["_sqlx_migrations"])
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-no-fabricated-removals", psql(db + "-removals", db, "SELECT count(*)=0 FROM auth_security.credential_removals").strip() == "t")
                check(db + "-private", psql(db + "-private-acl", db, "SELECT NOT has_schema_privilege('console_rt','auth_security','USAGE') AND NOT has_table_privilege('console_rt','auth_security.credential_removals','SELECT,INSERT,UPDATE,DELETE,TRUNCATE')").strip() == "t")
                post_schema = canonical_dump(schema(db + "-after-schema", db))
                run(db + "-topology-reapply", ["bash", str(ROOT / "backend-fork/ops/postgres-reconcile-topology.sh")], {
                    **passwords, "POSTGRES_HOST": str(socket), "POSTGRES_DB": db,
                    "POSTGRES_ADMIN_USER": admin, "POSTGRES_LOCAL_SOCKET_DIR": str(socket),
                    "CONSOLE_TOPOLOGY_REQUIRE_CANONICAL_TABLES": "1",
                })
                check(db + "-topology-schema-acl-preserved", canonical_dump(schema(db + "-reapplied-schema", db)) == post_schema)
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)

            def interrupt_auth_state_suffix():
                db = "auth_state_suffix_interrupt"
                previous = args.auth_state_baseline_app.resolve(strict=True)
                digest = hashlib.sha256(previous.read_bytes()).hexdigest()
                report["auth_state_baseline_app_sha256"] = digest
                check(db + "-accepted-installer", digest == "43cb7008a5b1150dcee45a45b3849bc54031e83e8e03422ca5f1669eb5a69b93")
                create(db)
                migrate(db + "-baseline", db, previous)
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000033','auth-state-client','State fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000034','Legacy key','12340000-0000-0000-0000-000000000033',ARRAY['MEMBER']),('12340000-0000-0000-0000-000000000035','Unknown legacy','12340000-0000-0000-0000-000000000033',ARRAY['MEMBER']); INSERT INTO auth_webauthn_credentials(id,user_id,org_id,credential_id,passkey_json) VALUES ('12340000-0000-0000-0000-000000000036','12340000-0000-0000-0000-000000000034','12340000-0000-0000-0000-000000000033','legacy-key','{}'); INSERT INTO auth_refresh_token_families(id,user_id,org_id,created_at) VALUES ('12340000-0000-0000-0000-000000000037','12340000-0000-0000-0000-000000000034','12340000-0000-0000-0000-000000000033',now());")
                before = rows(db + "-before", db)
                exact_ledger(db + "-prior-ledger", before["_sqlx_migrations"], [0, 226, 227, 228, 229])
                before_schema = canonical_dump(schema(db + "-before-schema", db))
                with ThreadPoolExecutor(max_workers=2) as workers:
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; LOCK TABLE public.auth_bootstrap_credentials IN ROW EXCLUSIVE MODE; SELECT pg_sleep(12); COMMIT;")
                    psql(db + "-lock-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_locks WHERE relation='public.auth_bootstrap_credentials'::regclass
                            AND mode='RowExclusiveLock' AND granted) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'OTP relation lock not acquired'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-cancel", db, """DO $$ DECLARE target integer; BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        SELECT a.pid INTO target FROM pg_stat_activity a JOIN pg_locks l ON l.pid=a.pid
                        WHERE a.datname=current_database() AND a.usename='console_app'
                          AND a.query LIKE '-- Expand only.%'
                          AND l.relation='public.auth_bootstrap_credentials'::regclass
                          AND l.mode='AccessExclusiveLock' AND NOT l.granted
                          AND cardinality(pg_blocking_pids(a.pid))>0 LIMIT 1;
                        IF target IS NOT NULL THEN
                            IF NOT pg_cancel_backend(target) THEN RAISE EXCEPTION 'cancel failed'; END IF;
                            RETURN;
                        END IF; PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual0230OTP boundary not reached'; END $$;""")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-schema-acl-rolled-back", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                def old_columns_preserved():
                    for table, old_rows in before.items():
                        if table == "_sqlx_migrations":
                            continue
                        current = after.get(table, [])
                        if len(current) != len(old_rows):
                            return False
                        if old_rows:
                            keys = old_rows[0].keys()
                            projected = sorted(({key: row[key] for key in keys} for row in current), key=lambda row: json.dumps(row, sort_keys=True))
                            if projected != old_rows:
                                return False
                    return True
                check(db + "-all-existing-rows-preserved", old_columns_preserved())
                check(db + "-prior-ledger-exact", after["_sqlx_migrations"][:len(before["_sqlx_migrations"])] == before["_sqlx_migrations"])
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-conservative-classification", psql(db + "-classification", db, "SELECT count(*)=2 AND count(*) FILTER (WHERE status='legacy_key_pending' AND ever_enrolled IS TRUE)=1 AND count(*) FILTER (WHERE status='unresolved' AND ever_enrolled IS NULL)=1 FROM auth_security.account_state").strip() == "t")
                check(db + "-legacy-family-unbound", psql(db + "-family-provenance", db, "SELECT provenance_version=0 AND auth_generation IS NULL AND session_purpose IS NULL AND source_kind IS NULL AND source_operation_id IS NULL FROM auth_refresh_token_families WHERE id='12340000-0000-0000-0000-000000000037'").strip() == "t")
                check(db + "-private", psql(db + "-private-acl", db, "SELECT NOT has_schema_privilege('console_rt','auth_security','USAGE') AND NOT has_table_privilege('console_rt','auth_security.account_state','SELECT,INSERT,UPDATE,DELETE,TRUNCATE')").strip() == "t")
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)

            def interrupt_auth_admission_suffix():
                db = "auth_admission_suffix_interrupt"
                previous = args.auth_state_baseline_app.resolve(strict=True)
                create(db)
                migrate(db + "-baseline", db, previous)
                psql(db + "-fixture", db, "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000043','auth-admission-client','Admission fixture'); INSERT INTO users(id,display_name,org_id,roles) VALUES ('12340000-0000-0000-0000-000000000044','Legacy account','12340000-0000-0000-0000-000000000043',ARRAY['MEMBER']);")
                prior = rows(db + "-prior", db)
                exact_ledger(db + "-prior-ledger", prior["_sqlx_migrations"], [0, 226, 227, 228, 229])
                with ThreadPoolExecutor(max_workers=2) as workers:
                    holder = workers.submit(psql, db + "-hold", db,
                        "BEGIN; CREATE TABLE auth_security.registration_intents (marker integer); SELECT pg_sleep(12); ROLLBACK;")
                    psql(db + "-holder-ready", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        IF EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                          AND usename='console_buck_admin' AND query LIKE 'SELECT pg_sleep(12)%') THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'uncommitted relation collision not established'; END $$;""")
                    running = workers.submit(migrate, db + "-apply", db, app, True)
                    psql(db + "-wait", db, """DO $$ BEGIN FOR i IN 1..1000 LOOP
                        PERFORM pg_stat_clear_snapshot();
                        IF EXISTS(SELECT 1 FROM pg_stat_activity a WHERE a.datname=current_database()
                          AND a.usename='console_app' AND a.query LIKE '-- Private admission custody.%'
                          AND cardinality(pg_blocking_pids(a.pid))>0) THEN RETURN; END IF;
                        PERFORM pg_sleep(0.005);
                        END LOOP; RAISE EXCEPTION 'actual0231 relation collision not reached'; END $$;""")
                    check(db + "-0230-committed", psql(db + "-0230-ledger", db,
                        "SELECT count(*)=1 FROM _sqlx_migrations WHERE version=230 AND success").strip() == "t")
                    before = rows(db + "-blocked", db)
                    before_schema = canonical_dump(schema(db + "-blocked-schema", db))
                    check(db + "-0231-uncommitted", 231 not in [row["version"] for row in before["_sqlx_migrations"]]
                          and "auth_security.session_admissions" not in before
                          and "auth_security.registration_intents" not in before)
                    check(db + "-cancel-sent", psql(db + "-cancel", db,
                        "SELECT pg_cancel_backend(a.pid) FROM pg_stat_activity a WHERE a.datname=current_database() AND a.usename='console_app' AND a.query LIKE '-- Private admission custody.%' AND cardinality(pg_blocking_pids(a.pid))>0").strip() == "t")
                    check(db + "-process-failed", running.result() != 0)
                    holder.result()
                check(db + "-rows-and-ledger-rolled-back", rows(db + "-interrupted", db) == before)
                check(db + "-schema-acl-rolled-back", canonical_dump(schema(db + "-interrupted-schema", db)) == before_schema)
                migrate(db + "-retry", db)
                after = rows(db + "-after", db)
                def old_columns_preserved():
                    for table, old_rows in prior.items():
                        if table == "_sqlx_migrations":
                            continue
                        current = after[table]
                        if len(current) != len(old_rows):
                            return False
                        if old_rows:
                            keys = old_rows[0].keys()
                            projected = sorted(({key: row[key] for key in keys} for row in current), key=lambda row: json.dumps(row, sort_keys=True))
                            if projected != old_rows:
                                return False
                    return True
                check(db + "-prior-rows-preserved", old_columns_preserved())
                check(db + "-legacy-account-preserved", psql(db + "-account", db,
                    "SELECT count(*)=1 FROM users WHERE id='12340000-0000-0000-0000-000000000044'").strip() == "t")
                exact_ledger(db + "-retry-ledger", after["_sqlx_migrations"], fresh_versions)
                check(db + "-private", psql(db + "-private-acl", db,
                    "SELECT NOT has_table_privilege('console_rt','auth_security.session_admissions','SELECT,INSERT,UPDATE,DELETE,TRUNCATE') AND NOT has_table_privilege('console_rt','auth_security.registration_intents','SELECT,INSERT,UPDATE,DELETE,TRUNCATE')").strip() == "t")
                migrate(db + "-repeat", db)
                check(db + "-repeat-preserves-state", rows(db + "-repeated", db) == after)

            try:
                run("start", ctl + ["-l", str(evidence / "postgres.log"), "-o", f"-c listen_addresses='' -c unix_socket_directories={socket}", "-w", "start"])
                create("reference")
                psql("historical-replay", "reference", historical_script(ROOT), user="console_app")
                reference_rows = rows("reference-rows", "reference")
                reference_schema = schema("reference-schema", "reference")
                generated = baseline_sql(reference_schema, reference_rows)
                if args.generate_baseline:
                    target = ROOT / BASELINE_PATH
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(generated)
                    report.update(status="generated", baseline_sha256=hashlib.sha256(generated.encode()).hexdigest())
                else:
                    check("baseline-reproduces-frozen-schema-and-catalogs", (ROOT / BASELINE_PATH).read_text() == generated)
                    migrate("reference-vendor-reconciliation", "reference")
                    reference_schema = schema("reference-with-vendor-schema", "reference")
                    # pg_dump expands BETWEEN to AND; PostgreSQL flattens the AND
                    # when reparsing. Compare one actual roundtrip of the reference,
                    # retaining every CHECK, function body, ACL and owner in the dump.
                    create("roundtrip")
                    psql("reference-roundtrip", "roundtrip", "BEGIN; SET LOCAL check_function_bodies=false;\n" + canonical_dump(reference_schema) + "\nCOMMIT;", user="console_app")
                    reference_schema = schema("roundtrip-schema", "roundtrip")
                    create("frontend_v1_clean")
                    # Exercise a pre-existing default grant, not only a pristine role setup.
                    psql("inherited-default-grant", "frontend_v1_clean", "ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO console_rt;")
                    with ThreadPoolExecutor(max_workers=2) as workers:
                        list(workers.map(lambda i: migrate(f"migrate-concurrent-{i}", "frontend_v1_clean"), range(2)))
                    migrate("migrate-repeat", "frontend_v1_clean")
                    fresh_rows = rows("fresh-rows", "frontend_v1_clean")
                    report["counts"] = {"organizations": len(fresh_rows["organizations"]), "accounts": len(fresh_rows["users"]), "employees": len(fresh_rows["employees"]), "applied_migrations": len(fresh_rows["_sqlx_migrations"])}
                    business = {table: len(values) for table, values in fresh_rows.items() if table not in (*CATALOGS, "_sqlx_migrations")}
                    report["business_table_counts"] = business
                    check("all-business-tables-empty", all(count == 0 for count in business.values()))
                    check("baseline-ledger-only", [r["version"] for r in fresh_rows["_sqlx_migrations"]] == fresh_versions)
                    exact_ledger("fresh-lineage", fresh_rows["_sqlx_migrations"], fresh_versions)
                    for table in CATALOGS:
                        check("catalog-" + table, catalog_rows(fresh_rows[table]) == catalog_rows(reference_rows[table]))
                    check("schema-owners-grants-rls-match", canonical_dump(schema("fresh-schema", "frontend_v1_clean")) == canonical_dump(reference_schema))
                    psql("inherited-column-grant", "frontend_v1_clean", "GRANT UPDATE(success), SELECT(version) ON public._sqlx_migrations TO console_rt;")
                    migrate("column-grant-reconciliation", "frontend_v1_clean")
                    for verb in ("DELETE FROM", "UPDATE"):
                        sql = "DELETE FROM _sqlx_migrations WHERE version=0" if verb.startswith("DELETE") else "UPDATE _sqlx_migrations SET success=false WHERE version=0"
                        output = psql("runtime-ledger-" + verb.split()[0], "frontend_v1_clean", sql, user="console_rt", allow_failure=True)
                        check("runtime-ledger-denies-" + verb.split()[0], "permission denied" in output)
                    if args.legacy_app:
                        legacy = args.legacy_app.resolve(strict=True)
                        report["legacy_app_sha256"] = hashlib.sha256(legacy.read_bytes()).hexdigest()
                        check("old-runner-refuses-baseline", migrate("old-runner", "frontend_v1_clean", legacy, True) != 0)
                        check("old-runner-preserves-data", rows("after-old-runner", "frontend_v1_clean") == fresh_rows)
                    else:
                        report["checks"].append({"name": "old-runner-refuses-baseline", "status": "unreached"})

                    if args.baseline_app:
                        baseline_app = args.baseline_app.resolve(strict=True)
                        report["baseline_app_sha256"] = hashlib.sha256(baseline_app.read_bytes()).hexdigest()
                        create("installed_baseline")
                        migrate("installed-baseline-initial", "installed_baseline", baseline_app)
                        initial = rows("installed-baseline-initial-rows", "installed_baseline")
                        exact_ledger("installed-baseline-original", initial["_sqlx_migrations"], [0])
                        check("installed-baseline-initial-empty", all(not data for table, data in initial.items() if table not in (*CATALOGS, "_sqlx_migrations")))
                        psql("installed-baseline-fixture", "installed_baseline", "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000002','baseline-client','Baseline upgrade fixture'); INSERT INTO users(display_name,org_id,roles) VALUES ('Existing baseline account','12340000-0000-0000-0000-000000000002',ARRAY['MEMBER']);")
                        prior = rows("installed-baseline-before", "installed_baseline")
                        check("installed-baseline-populated", len(prior["users"]) == 1 and len(prior["organizations"]) == 1)
                        migrate("installed-baseline-upgrade", "installed_baseline")
                        upgraded = rows("installed-baseline-after", "installed_baseline")
                        for table, values in prior.items():
                            if table != "_sqlx_migrations":
                                check("installed-baseline-preserves-" + table, values == upgraded[table])
                        exact_ledger("installed-baseline-final", upgraded["_sqlx_migrations"], fresh_versions)
                        check("installed-baseline-preserves-ledger", upgraded["_sqlx_migrations"][:1] == prior["_sqlx_migrations"])
                        if 230 in report["source_closure"]["suffix_versions"]:
                            check("installed-baseline-legacy-auth-unresolved", psql("installed-baseline-auth-state", "installed_baseline", "SELECT count(*)=(SELECT count(*) FROM users) AND bool_and(status='unresolved' AND ever_enrolled IS NULL AND auth_generation=1) FROM auth_security.account_state").strip() == "t")
                        migrate("installed-baseline-repeat", "installed_baseline")
                        check("installed-baseline-repeat-exact-state", rows("installed-baseline-repeat-rows", "installed_baseline") == upgraded)
                    else:
                        report["checks"].append({"name": "installed-baseline-upgrade", "status": "unreached"})

                    # Upgrade the preceding populated historical version; names resembling
                    # bootstrap records are deliberately retained as legitimate legacy rows.
                    create("upgrade")
                    psql("historical-through-224", "upgrade", historical_script(ROOT, through=224), user="console_app")
                    psql("legacy-fixture", "upgrade", "INSERT INTO organizations(id,slug,name) VALUES ('12340000-0000-0000-0000-000000000001','legacy-client','Cold Start Admin'); INSERT INTO users(display_name,org_id,roles) VALUES ('Cold Start Admin','12340000-0000-0000-0000-000000000001',ARRAY['MEMBER']);")
                    before = rows("upgrade-before", "upgrade")
                    psql("tamper-late-historical-checksum", "upgrade", "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=224")
                    tampered = rows("tampered-legacy-before", "upgrade")
                    check("late-historical-checksum-refused", migrate("tampered-legacy", "upgrade", allow_failure=True) != 0)
                    check("late-checksum-no-pending-sql", rows("tampered-legacy-after", "upgrade") == tampered)
                    checksum = next(row["checksum"] for row in before["_sqlx_migrations"] if row["version"] == 224)[2:]
                    psql("restore-late-checksum", "upgrade", f"UPDATE _sqlx_migrations SET checksum=decode('{checksum}','hex') WHERE version=224")
                    migrate("upgrade", "upgrade")
                    after = rows("upgrade-after", "upgrade")
                    # 225 explicitly adds Group-of-one and changes organizations.updated_at.
                    for table, values in before.items():
                        if table not in ("organizations", "groups", "group_memberships", "_sqlx_migrations"):
                            check("upgrade-preserves-" + table, sorted(values, key=str) == sorted(after[table], key=str))
                    for row in before["organizations"]:
                        current = next(r for r in after["organizations"] if r["id"] == row["id"])
                        check("upgrade-preserves-org-" + row["id"], all(current[k] == v for k, v in row.items() if k not in ("group_id", "updated_at")))
                    check("upgrade-preserves-historical-ledger", after["_sqlx_migrations"][:224] == before["_sqlx_migrations"])
                    exact_ledger("historical-upgrade-lineage", after["_sqlx_migrations"], historical_versions)
                    if 230 in report["source_closure"]["suffix_versions"]:
                        check("historical-legacy-auth-unresolved", psql("historical-auth-state", "upgrade", "SELECT count(*)=(SELECT count(*) FROM users) AND bool_and(status='unresolved' AND ever_enrolled IS NULL AND auth_generation=1) FROM auth_security.account_state").strip() == "t")
                    migrate("upgrade-repeat", "upgrade")
                    check("upgrade-repeat-exact-state", rows("upgrade-repeat-rows", "upgrade") == after)

                    # A removed seed-migration ledger row must not be replayed.
                    psql("make-ledger-gap", "upgrade", "DELETE FROM _sqlx_migrations WHERE version=21")
                    gap = rows("gap-before", "upgrade")
                    check("ledger-gap-refused", migrate("ledger-gap", "upgrade", allow_failure=True) != 0)
                    check("ledger-gap-no-writes", rows("gap-after", "upgrade") == gap)
                    psql("tamper-baseline", "frontend_v1_clean", "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=0")
                    check("baseline-tampering-refused", migrate("tampered-baseline", "frontend_v1_clean", allow_failure=True) != 0)
                    psql("restore-baseline-checksum", "frontend_v1_clean", f"UPDATE _sqlx_migrations SET checksum=decode('{hashlib.sha384(generated.encode()).hexdigest()}','hex') WHERE version=0")
                    migrate("baseline-checksum-restored", "frontend_v1_clean")

                    create("occupied")
                    for kind, install, inspect, remove in (
                        ("function", "CREATE FUNCTION public.keep_me() RETURNS integer LANGUAGE sql AS 'SELECT 7'", "SELECT keep_me()=7", "DROP FUNCTION public.keep_me()"),
                        ("table", "CREATE TABLE keep_me(value integer); INSERT INTO keep_me VALUES(7)", "SELECT value=7 FROM keep_me", "DROP TABLE keep_me"),
                        ("sequence", "CREATE SEQUENCE keep_me", "SELECT to_regclass('keep_me') IS NOT NULL", "DROP SEQUENCE keep_me"),
                        ("type", "CREATE TYPE keep_me AS ENUM ('kept')", "SELECT 'kept'::keep_me::text='kept'", "DROP TYPE keep_me"),
                        ("schema", "CREATE SCHEMA keep_me", "SELECT to_regnamespace('keep_me') IS NOT NULL", "DROP SCHEMA keep_me"),
                        ("pgx-schema", "CREATE SCHEMA pgx; CREATE TABLE pgx.keep_me(value integer); INSERT INTO pgx.keep_me VALUES (7)", "SELECT value=7 FROM pgx.keep_me", "DROP SCHEMA pgx CASCADE"),
                        ("extension", "CREATE EXTENSION pg_trgm", "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='pg_trgm')", "DROP EXTENSION pg_trgm"),
                        ("large-object", "SELECT lo_create(42)", "SELECT EXISTS(SELECT 1 FROM pg_largeobject_metadata WHERE oid=42)", "SELECT lo_unlink(42)"),
                        ("default-function-grant", "ALTER DEFAULT PRIVILEGES FOR ROLE console_app GRANT EXECUTE ON FUNCTIONS TO console_rt", "SELECT EXISTS(SELECT 1 FROM pg_default_acl WHERE defaclobjtype='f')", "ALTER DEFAULT PRIVILEGES FOR ROLE console_app REVOKE EXECUTE ON FUNCTIONS FROM console_rt"),
                    ):
                        psql("foreign-" + kind, "occupied", install)
                        check("foreign-" + kind + "-refused", migrate("occupied-" + kind, "occupied", allow_failure=True) != 0)
                        check("foreign-" + kind + "-preserved", psql("foreign-" + kind + "-read", "occupied", inspect).strip() == "t")
                        psql("remove-test-" + kind, "occupied", remove)
                    migrate("occupied-retry", "occupied")
                    check("retry-empty", all(not data for table, data in rows("retry-rows", "occupied").items() if table not in (*CATALOGS, "_sqlx_migrations")))
                    interrupt_apply("before_commit")
                    interrupt_apply("after_commit", after_commit=True)
                    if 226 in report["source_closure"]["suffix_versions"]:
                        if args.baseline_app:
                            interrupt_credential_suffix()
                        else:
                            report["checks"].append({"name": "credential-suffix-interruption", "status": "unreached"})
                    if 227 in report["source_closure"]["suffix_versions"]:
                        if args.account_lock_baseline_app:
                            interrupt_account_lock_suffix()
                        else:
                            report["checks"].append({"name": "account-lock-suffix-interruption", "status": "unreached"})
                    if 228 in report["source_closure"]["suffix_versions"]:
                        if args.identity_baseline_app:
                            interrupt_identity_suffix()
                        else:
                            report["checks"].append({"name": "identity-suffix-interruption", "status": "unreached"})
                    if 229 in report["source_closure"]["suffix_versions"]:
                        if args.removal_baseline_app:
                            interrupt_removal_suffix()
                        else:
                            report["checks"].append({"name": "removal-suffix-interruption", "status": "unreached"})
                    if 230 in report["source_closure"]["suffix_versions"]:
                        if args.auth_state_baseline_app:
                            interrupt_auth_state_suffix()
                        else:
                            report["checks"].append({"name": "auth-state-suffix-interruption", "status": "unreached"})
                    if 231 in report["source_closure"]["suffix_versions"]:
                        if args.auth_state_baseline_app:
                            interrupt_auth_admission_suffix()
                        else:
                            report["checks"].append({"name": "auth-admission-suffix-interruption", "status": "unreached"})
                    report["status"] = "passed"

                    test_url = "postgresql://" + admin + ":" + passwords["POSTGRES_ADMIN_PASSWORD"] + "@localhost/frontend_v1_clean?" + urlencode({"host": str(socket), "options[console.sqlx_test_bootstrap]": "buck-sqlx-superuser-v1"})
                    if args.cargo_tests or args.cargo_integration:
                        cargo = Path(os.environ["HOME"]) / ".cargo/bin/cargo"
                        targets = [("cargo-postgres", ["--workspace", "--lib"])] if args.cargo_tests else []
                        for target in args.cargo_integration:
                            package, name = target.split(":", 1)
                            if not all(part and all(c.isalnum() or c in "_-" for c in part) for part in (package, name)):
                                raise ValueError("invalid integration target")
                            targets.append(("cargo-" + package + "-" + name, ["-p", package, "--test", name]))
                        report["cargo_status"] = "passed"
                        report["cargo_suites"] = []
                        for label, selection in targets:
                            base = [str(cargo), "test", *selection, "--locked", "--offline", "--no-fail-fast", "-j", "2", "--target-dir", str(ROOT / ".artifacts/cargo-target")]
                            discovery = run(label + "-discovery", base + ["--", "--list"], {"DATABASE_URL": test_url, "SQLX_OFFLINE": "true", "CONSOLE_TEST_RUNTIME_PASSWORD": passwords["CONSOLE_RT_POSTGRES_PASSWORD"], "CONSOLE_TEST_FORCE_PASSWORD": passwords["CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD"]}, capture=True, timeout=1800, cwd=ROOT / "backend-fork/backend", allow_failure=True)
                            status = run(label, base + ["--", "--test-threads=1"], {"DATABASE_URL": test_url, "SQLX_OFFLINE": "true", "CONSOLE_TEST_RUNTIME_PASSWORD": passwords["CONSOLE_RT_POSTGRES_PASSWORD"], "CONSOLE_TEST_FORCE_PASSWORD": passwords["CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD"]}, timeout=1800, cwd=ROOT / "backend-fork/backend", allow_failure=True)
                            counts = {**cargo_counts(discovery, discovery=True), **cargo_counts((evidence / (label + ".log")).read_text())}
                            complete = counts["discovered"] > 0 and counts["executed"] == counts["discovered"] and counts["skipped"] == 0
                            report["cargo_suites"].append({"name": label, "status": "passed" if status == 0 and complete else "failed", **counts})
                            if not complete:
                                status = 1
                            if status:
                                report["cargo_status"] = "failed"
                                report["status"] = "failed"
                    if args.migration_build_probe:
                        output = run("migration-build-probe", [sys.executable, str(ROOT / "tools/check-migration-build.py")], {"DATABASE_URL": test_url}, capture=True, timeout=1800, allow_failure=True)
                        summary = json.loads(output)
                        probe_path = ROOT / summary["evidence"]
                        probe = json.loads(probe_path.read_text())
                        report["migration_build_probe"] = {"path": summary["evidence"], "sha256": hashlib.sha256(probe_path.read_bytes()).hexdigest(), "status": probe["status"], "phases": probe["phases"]}
                        if probe["status"] != "passed":
                            report["status"] = "failed"
            finally:
                subprocess.run(ctl + ["-m", "immediate", "-w", "stop"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
    except (OSError, RuntimeError, ValueError, AssertionError, subprocess.TimeoutExpired) as error:
        report.update(status="failed", error=str(error))
    report["executed_checks"] = len([c for c in report["checks"] if c["status"] != "unreached"])
    report["unreached_checks"] = len([c for c in report["checks"] if c["status"] == "unreached"])
    (evidence / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "counts": report.get("counts"), "evidence": str(evidence.relative_to(ROOT) / "report.json")}))
    return 0 if report["status"] in ("passed", "generated") else 1


if __name__ == "__main__":
    raise SystemExit(main())
