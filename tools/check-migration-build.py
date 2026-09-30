#!/usr/bin/env python3
"""Exercise real incremental SQLx embedding in a disposable, fixed-source workspace.

DATABASE_URL must identify the disposable SQLx test administrator supplied by
check-empty-install.py. No business migrations or production lockfiles are edited.
"""
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
from urllib.parse import unquote, urlsplit

from acceptance import cargo_counts

ROOT = Path(__file__).resolve().parents[1]
BACKEND = ROOT / "backend-fork/backend"
DB = BACKEND / "crates/platform/db"
PHASES = ("initial", "unchanged", "add-migration", "edit-migration", "rename-migration", "remove-migration", "add-baseline", "edit-baseline", "remove-baseline", "final-unchanged")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def protected_sources():
    paths = [BACKEND / "Cargo.lock", *DB.joinpath("migrations").glob("*.sql"), *DB.joinpath("baseline").glob("*.sql")]
    return {str(p.relative_to(ROOT)): digest(p) for p in sorted(paths)}


def consumer_census():
    """Conservative source census; counts are macro sites, not executed tests."""
    packages = {}
    for path in BACKEND.rglob("Cargo.toml"):
        document = tomllib.loads(path.read_text())
        if "package" in document:
            packages[document["package"]["name"]] = (path.parent, document)
    edges = {}
    dev_edges = {}
    consumers = {}
    for name, (folder, doc) in packages.items():
        def local_edges(section):
            return {value.get("package", key) if isinstance(value, dict) else key
                    for key, value in doc.get(section, {}).items()
                    if not isinstance(value, dict) or not value.get("optional", False)} & packages.keys()
        edges[name] = local_edges("dependencies")
        dev_edges[name] = local_edges("dev-dependencies")
        sites = []
        for path in sorted(folder.rglob("*.rs")):
            # Do not silently absorb another package's source under this one.
            if any(parent != folder and parent in path.parents for parent, _ in packages.values()):
                continue
            text = path.read_text()
            for match in re.finditer(r"sqlx::migrate!\s*\(|#\[sqlx::test(?:\([^\]]*\))?\]", text):
                line = text[text.rfind("\n", 0, match.start()) + 1:match.start()]
                if line.lstrip().startswith("//") or re.search(r"migrations\s*=\s*false", match.group()):
                    continue
                sites.append({"path": str(path.relative_to(ROOT)), "line": text[:match.start()].count("\n") + 1, "test_only": "tests" in path.relative_to(folder).parts or match.group().startswith("#[sqlx::test")})
        if sites:
            consumers[name] = sites
    def reaches_db(name, include_dev=False, seen=None):
        if name == "console-platform-db":
            return True
        seen = set() if seen is None else seen
        if name in seen:
            return False
        seen.add(name)
        return any(reaches_db(child, False, seen.copy()) for child in (edges[name] | (dev_edges[name] if include_dev else set())))
    authz_build = packages["console-platform-authz"][1]["package"].get("build")
    watcher = DB / "build.rs"
    covered = {name: watcher.is_file() and (all(reaches_db(name, site["test_only"]) for site in consumers[name]) or (name == "console-platform-authz" and authz_build == "../db/build.rs")) for name in consumers}
    return {"packages_inspected": len(packages), "consumer_packages": len(consumers), "sites": consumers,
            "covered": covered, "uncovered": sorted(name for name, good in covered.items() if not good)}


RUST = '''pub fn marker() { DEPENDENCY }
pub fn inventory() -> String {
    marker();
    let mut result = String::new();
    for (kind, migrations) in [("migrations", sqlx::migrate!("../db/migrations")), ("baseline", sqlx::migrate!("../db/baseline"))] {
        for migration in migrations.iter() {
            let checksum: String = migration.checksum.iter().map(|byte| format!("{byte:02x}")).collect();
            result.push_str(&format!("{kind}|{}|{}|{checksum}|{}\\n{}\\n", migration.version, migration.description, migration.sql.as_str().len(), migration.sql.as_str()));
        }
    }
    result
}
#[test]
fn embedded_inventory_matches_files() {
    assert_eq!(inventory(), std::env::var("BUILD_EXPECTED_INVENTORY").unwrap());
}
ATTRIBUTE
async fn attribute_installs_current_sources(pool: sqlx::PgPool) {
    marker();
    let rows: Vec<(i64, String, Vec<u8>)> = sqlx::query_as("SELECT version,description,checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&pool).await.unwrap();
    let ledger: String = rows.iter().map(|(version, description, hash)| {
        let hash: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("{version}|{description}|{hash}\\n")
    }).collect();
    assert_eq!(ledger, std::env::var("BUILD_EXPECTED_LEDGER").unwrap());
    let values: Vec<i32> = sqlx::query_scalar("SELECT value FROM build_probe ORDER BY value").fetch_all(&pool).await.unwrap();
    let expected: Vec<i32> = std::env::var("BUILD_EXPECTED_VALUES").unwrap().split(',').map(|v| v.parse().unwrap()).collect();
    assert_eq!(values, expected);
}
'''


def main():
    source_before = protected_sources()
    evidence = ROOT / ".artifacts" / datetime.now(timezone.utc).strftime("migration-build-%Y%m%dT%H%M%S.%fZ")
    fixture = evidence / "fixture"
    fixture.mkdir(parents=True)
    report = {"scope": "incremental build dependency proof, not production schema/business acceptance", "status": "unreached", "commands": [], "checks": [], "phases": [{"name": name, "status": "unreached"} for name in PHASES], "source_before": source_before, "census": consumer_census()}
    env = dict(os.environ)
    url = env["DATABASE_URL"]
    secret = unquote(urlsplit(url).password or "")
    env["SQLX_OFFLINE"] = "true"
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env.pop("RUSTFLAGS", None)
    cargo = shutil.which("cargo")
    if not cargo:
        raise RuntimeError("cargo is required")

    def run(name, args, runtime=None, allow_failure=False):
        result = subprocess.run(args, cwd=fixture, env={**env, **(runtime or {})}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=1800)
        raw = result.stdout.replace(secret.encode(), b"[REDACTED]") if secret else result.stdout
        path = evidence / (name + ".log")
        path.write_bytes(raw)
        report["commands"].append({"name": name, "command": [str(a) for a in args], "exit_code": result.returncode, "output_sha256": digest(path)})
        if result.returncode and not allow_failure:
            raise RuntimeError(name + " failed; see " + str(path.relative_to(ROOT)))
        return raw.decode(), result.returncode

    def check(name, condition):
        report["checks"].append({"name": name, "status": "passed" if condition else "failed"})
        return condition

    def fixture_static():
        return {str(p.relative_to(fixture)): digest(p) for p in fixture.rglob("*") if p.is_file() and (p.suffix == ".rs" or p.name in ("Cargo.toml", "Cargo.lock"))}

    def expected():
        inventory, ledger = "", ""
        for kind in ("migrations", "baseline"):
            for file in sorted((fixture / "db" / kind).glob("*.sql")):
                version, name = file.stem.split("_", 1)
                sql = file.read_text()
                checksum = hashlib.sha384(file.read_bytes()).hexdigest()
                row = f"{int(version)}|{name.replace('_', ' ')}|{checksum}"
                inventory += f"{kind}|{row}|{len(file.read_bytes())}\n{sql}\n"
                if kind == "migrations":
                    ledger += row + "\n"
        values = []
        for file in (fixture / "db/migrations").glob("*.sql"):
            values.extend(int(v) for v in re.findall(r"INSERT INTO build_probe VALUES\((\d+)\)", file.read_text()))
        return {"BUILD_EXPECTED_INVENTORY": inventory, "BUILD_EXPECTED_LEDGER": ledger, "BUILD_EXPECTED_VALUES": ",".join(map(str, sorted(values)))}

    try:
        production_lock = tomllib.loads((BACKEND / "Cargo.lock").read_text())
        version = {p["name"]: p["version"] for p in production_lock["package"] if p["name"] in ("sqlx", "tokio")}
        assert version["sqlx"] == "0.9.0"
        common = f'sqlx = {{ version = "={version["sqlx"]}", default-features = false, features = ["runtime-tokio","postgres","uuid","time","json","migrate","macros"] }}\ntokio = {{ version = "={version["tokio"]}", features = ["macros","rt-multi-thread"] }}\n'
        (fixture / "Cargo.toml").write_text('[workspace]\nresolver = "3"\nmembers = ["db","direct","app","dev","authz"]\n[profile.dev]\ndebug = 0\n')
        authz = tomllib.loads((BACKEND / "crates/platform/authz/Cargo.toml").read_text())["package"]
        for name, dependency in (("db", ""), ("direct", "build_db::marker();"), ("app", "build_direct::marker();"), ("dev", "build_db::marker();"), ("authz", "")):
            folder = fixture / name
            (folder / "src").mkdir(parents=True)
            manifest = f'[package]\nname = "build-{name}"\nversion = "0.0.0"\nedition = "2024"\n'
            if name == "authz" and authz.get("build"):
                assert authz["build"] == "../db/build.rs", "review new external build path"
                manifest += 'build = "../db/build.rs"\n'
            manifest += "[dev-dependencies]\n" if name == "dev" else "[dependencies]\n"
            manifest += common
            if name in ("direct", "dev"):
                manifest += 'build-db = { path = "../db" }\n'
            elif name == "app":
                manifest += 'build-direct = { path = "../direct" }\n'
            (folder / "Cargo.toml").write_text(manifest)
            attribute = '#[sqlx::test]' if name == "db" else '#[sqlx::test(migrations = "../db/migrations")]'
            code = RUST.replace("DEPENDENCY", dependency).replace("ATTRIBUTE", attribute)
            if name == "dev":
                (folder / "src/lib.rs").write_text("// Dev-only migration consumer.\n")
                (folder / "tests").mkdir()
                (folder / "tests/embedding.rs").write_text(code)
            else:
                (folder / "src/lib.rs").write_text(code)
            if name == "app":
                (folder / "src/main.rs").write_text('fn main() { print!("{}", build_app::inventory()); }\n')
        (fixture / "db/migrations").mkdir()
        (fixture / "db/baseline").mkdir()
        (fixture / "db/migrations/0001_initial.sql").write_text("CREATE TABLE build_probe(value integer PRIMARY KEY); INSERT INTO build_probe VALUES(1);\n")
        (fixture / "db/baseline/0000_initial.sql").write_text("SELECT 0;\n")
        if (DB / "build.rs").is_file():
            shutil.copy2(DB / "build.rs", fixture / "db/build.rs")
        shutil.copy2(BACKEND / "Cargo.lock", fixture / "Cargo.lock")
        run("fixture-only-lock", [cargo, "metadata", "--offline", "--format-version=1"])
        original_registry = {(p["name"], p["version"], p.get("source")): p.get("checksum") for p in production_lock["package"] if p.get("source")}
        resolved = tomllib.loads((fixture / "Cargo.lock").read_text())
        check("fixture-registry-identities-match-production", all((p["name"], p["version"], p["source"]) in original_registry and original_registry[(p["name"], p["version"], p["source"])] == p.get("checksum") for p in resolved["package"] if p.get("source")))
        fixed = fixture_static()
        report["fixture_fixed_sources"] = fixed
        target = evidence / "target"
        for phase_info in report["phases"]:
            phase = phase_info["name"]
            phase_info["status"] = "running"
            if phase == "add-migration":
                (fixture / "db/migrations/0002_added.sql").write_text("INSERT INTO build_probe VALUES(2);\n")
            elif phase == "edit-migration":
                (fixture / "db/migrations/0002_added.sql").write_text("INSERT INTO build_probe VALUES(20);\n")
            elif phase == "rename-migration":
                (fixture / "db/migrations/0002_added.sql").rename(fixture / "db/migrations/0002_renamed.sql")
            elif phase == "remove-migration":
                (fixture / "db/migrations/0002_renamed.sql").unlink()
            elif phase == "add-baseline":
                (fixture / "db/baseline/0001_added.sql").write_text("SELECT 1;\n")
            elif phase == "edit-baseline":
                (fixture / "db/baseline/0001_added.sql").write_text("SELECT 10;\n")
            elif phase == "remove-baseline":
                (fixture / "db/baseline/0001_added.sql").unlink()
            runtime = expected()
            before_checks = len(report["checks"])
            normal, _ = run(phase + "-production-build", [cargo, "build", "-p", "build-app", "--locked", "--offline", "--target-dir", str(target), "--message-format=json"])
            artifacts, _ = run(phase + "-test-build", [cargo, "test", "--workspace", "--all-targets", "--no-run", "--locked", "--offline", "--target-dir", str(target), "--message-format=json"])
            build_messages = [json.loads(line) for output in (normal, artifacts) for line in output.splitlines() if line.startswith('{"reason":')]
            workspace_artifacts = [m for m in build_messages if m.get("reason") == "compiler-artifact" and m["package_id"].startswith("path+")]
            if phase in ("unchanged", "final-unchanged"):
                check(phase + "-all-workspace-artifacts-fresh", bool(workspace_artifacts) and all(m["fresh"] for m in workspace_artifacts))
            report.setdefault("freshness", {})[phase] = [{"target": m["target"]["name"], "kind": m["target"]["kind"], "fresh": m["fresh"]} for m in workspace_artifacts]
            output, _ = run(phase + "-production-executable", [str(target / "debug/build-app")])
            check(phase + "-production-inventory", output == runtime["BUILD_EXPECTED_INVENTORY"])
            executables = sorted({m["executable"] for m in workspace_artifacts if m.get("executable") and m["profile"]["test"]})
            counts = {key: 0 for key in ("discovered", "executed", "passed", "failed", "skipped", "filtered")}
            for i, executable in enumerate(executables):
                listed, _ = run(f"{phase}-tests-{i}-discovery", [executable, "--list"], runtime)
                output, status = run(f"{phase}-tests-{i}", [executable, "--test-threads=1"], runtime, allow_failure=True)
                discovered = cargo_counts(listed, discovery=True)["discovered"]
                result = cargo_counts(output)
                counts["discovered"] += discovered
                for key in ("executed", "passed", "failed", "skipped", "filtered"):
                    counts[key] += result[key]
                check(f"{phase}-tests-{i}-passed", status == 0 and discovered == result["passed"] and not result["skipped"] and not result["filtered"])
            check(phase + "-ten-tests-reached", counts["discovered"] == counts["executed"] == 10)
            check(phase + "-fixed-rust-manifests-build-script-lock", fixture_static() == fixed)
            phase_ok = all(c["status"] == "passed" for c in report["checks"][before_checks:])
            phase_info.update({"status": "passed" if phase_ok else "failed", **counts,
                "sql": {str(p.relative_to(fixture)): digest(p) for p in (fixture / "db").rglob("*.sql")},
                "executables": {p: digest(Path(p)) for p in executables}})
            if not phase_ok:
                break
        check("all-repository-consumers-have-watcher-path", not report["census"]["uncovered"])
        report["status"] = "passed" if all(c["status"] == "passed" for c in report["checks"]) and all(p["status"] == "passed" for p in report["phases"]) else "failed"
    except Exception as error:
        report.update(status="failed", error=str(error))
    finally:
        for phase_info in report["phases"]:
            if phase_info["status"] == "running":
                phase_info["status"] = "failed"
        report["source_after"] = protected_sources()
        if not check("production-sql-and-lock-unchanged", report["source_after"] == source_before):
            report["status"] = "failed"
        (evidence / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "evidence": str((evidence / "report.json").relative_to(ROOT))}))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
