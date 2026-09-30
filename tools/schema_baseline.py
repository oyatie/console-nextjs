"""Reproducible schema snapshot helpers; used only on disposable test databases."""
import hashlib
import json
import re


BASE_VERSION = 225
# Only global configuration. Never infer this list from populated tables.
CATALOGS = (
    "feature_catalog", "lifecycle_transition_rules", "link_types", "object_types",
    "ont_builtin_catalog_allowlist", "payroll_statutory_rates",
)
BASELINE_PATH = "backend-fork/backend/crates/platform/db/baseline/0000_schema_at_0225.sql"


def canonical_dump(dump):
    """Remove pg_dump's random psql nonce, version banner and session preamble."""
    preamble, separator, body = dump.partition("-- Name:")
    assert separator, "pg_dump schema header missing"
    # Only the dump preamble is discarded; function bodies stay byte-exact.
    for line in ("SET default_tablespace = '';", "SET default_table_access_method = heap;"):
        assert body.count(line) == 1, "unexpected dump table defaults"
        body = body.replace(line, "")
    body = "\n".join(line for line in body.splitlines() if not line.startswith("\\unrestrict "))
    return "-- Name:" + body.strip() + "\n"


def catalog_rows(rows):
    return sorted(({k: v for k, v in row.items() if k != "created_at"} for row in rows),
                  key=lambda row: json.dumps(row, sort_keys=True, ensure_ascii=False))


def baseline_sql(dump, rows):
    schema = canonical_dump(dump)
    # public already belongs to the migration owner after topology reconciliation.
    assert "-- *not* creating schema, since initdb creates it" in schema
    schema = schema.replace("CREATE SCHEMA public;", "")
    result = [
        "-- Generated from the frozen historical schema through migration 225.\n"
        "-- Regenerate only with tools/check-empty-install.py --generate-baseline.\n"
        "-- No tenant, person, credential, group or other business rows are copied.\n"
        "SET LOCAL search_path = public;\nSET LOCAL check_function_bodies = false;\n"
        "SET LOCAL standard_conforming_strings = on;\nSET LOCAL row_security = off;\n"
        "ALTER DEFAULT PRIVILEGES FOR ROLE console_app IN SCHEMA public REVOKE ALL ON TABLES FROM console_rt;\n",
        schema,
    ]
    for table in CATALOGS:
        records = catalog_rows(rows[table])
        assert records, f"reference catalog missing: {table}"
        columns = list(records[0])
        encoded = json.dumps(records, ensure_ascii=False, separators=(",", ":"))
        assert "$catalog$" not in encoded
        result.append(f"INSERT INTO public.{table} ({', '.join(columns)})\n"
                      f"SELECT {', '.join(columns)} FROM json_populate_recordset(\n"
                      f"NULL::public.{table}, $catalog${encoded}$catalog$::json);\n")
    result.append("SET LOCAL check_function_bodies = true;\nSET LOCAL row_security = on;\n"
                  "SET LOCAL search_path = public;\n")
    return "\n".join(result)


def historical_script(root, through=BASE_VERSION):
    """Test-only replay with SQLx-compatible, truthful execution checksums."""
    manifest = json.loads((root / "backend-fork/SOURCE-CLOSURE.json").read_text())
    parts = ["CREATE TABLE _sqlx_migrations (version bigint PRIMARY KEY, description text NOT NULL, "
             "installed_on timestamptz NOT NULL DEFAULT now(), success boolean NOT NULL, "
             "checksum bytea NOT NULL, execution_time bigint NOT NULL);\n"]
    folder = root / "backend-fork/backend/crates/platform/db/migrations"
    for file in sorted(folder.glob("*.sql")):
        match = re.fullmatch(r"(\d+)_(.+)\.sql", file.name)
        assert match, file
        version = int(match[1])
        if version > through:
            continue
        raw = file.read_bytes()
        assert hashlib.sha256(raw).hexdigest() == manifest["files"][str(file.relative_to(root / "backend-fork"))]["sha256"]
        sql = raw.decode()
        no_tx = sql.startswith("-- no-transaction")
        description = match[2].replace("_", " ").replace("'", "''")
        if not no_tx:
            parts.append("BEGIN;")
        parts.append(sql)
        parts.append(f"INSERT INTO _sqlx_migrations VALUES ({version}, '{description}', now(), true, "
                     f"decode('{hashlib.sha384(raw).hexdigest()}', 'hex'), -1);")
        if not no_tx:
            parts.append("COMMIT;")
    return "\n".join(parts)
