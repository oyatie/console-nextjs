#!/usr/bin/env python3
"""Record v1 readiness gaps and optional real baseline commands; never infer release acceptance."""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time

from schema_baseline import BASELINE_PATH, BASE_VERSION


ROOT = Path(__file__).resolve().parents[1]
PLANNING = ROOT / "docs/planning/nextjs-v1"
MIGRATION_DIRECTORY = "backend/crates/platform/db/migrations"
BASELINE_SHA256 = "30fdb9d5cea24ddbded0de37c940dfbeb19478185259c8aa08f81f924b7ea1f1"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def preflight(root=ROOT):
    """Necessary static negatives, not substitutes for browser/protocol/fault tests."""
    probes = [
        ("no-plaintext-business-storage", "src/lib/store.ts", "localStorage.setItem"),
        ("no-api-success-fallback", "src/lib/api-client.ts", "data: fallbackData"),
        ("no-invented-secure-offline-mode", "src/lib/api-client.ts", 'mode: "AUTONOMOUS_SECURE"'),
        ("no-static-export", "next.config.mjs", 'output: "export"'),
        ("no-static-serving", "package.json", "serve out"),
    ]
    results = production_preflights([], root)
    for name, path, forbidden in probes:
        file = root / path
        if not file.exists():
            results.append({"id": name, "status": "unreached", "path": path, "reason": "input missing"})
            continue
        content = file.read_bytes()
        lines = [i for i, line in enumerate(content.decode().splitlines(), 1) if forbidden in line]
        results.append({"id": name, "status": "failed" if lines else "passed", "path": path, "input_sha256": sha(content), "lines": lines, "scope": "necessary static preflight only"})
    return results


def known_fixture_artifacts(root):
    """Check known development fixture literals in the current production output."""
    result = {
        "id": "known-business-fixture-artifacts-excluded",
        "status": "unreached",
        "scope": "known seed-data.ts literals and identifiers in current production route/asset bytes",
        "limits": "Bytewise known-marker scan; does not prove other or encoded seeds, backend responses, or database rows",
    }
    source = root / "src/lib/seed-data.ts"
    folders = [root / path for path in (".next/static", ".next/server", ".artifacts/production-runtime/.next/static", ".artifacts/production-runtime/.next/server")]
    server = root / ".artifacts/production-runtime/server.mjs"
    if (not source.is_file() or source.is_symlink() or not server.is_file() or server.is_symlink()
            or any(not folder.is_dir() or folder.is_symlink() for folder in folders)):
        result["reason"] = "Fixture source or production output missing"
        return result
    try:
        fixture = source.read_text()
        literals = set()
        for raw in re.findall(r'"(?:[^"\\]|\\.)*"', fixture):
            try:
                value = json.loads(raw)
            except ValueError:
                continue
            if len(value) >= 12 or re.fullmatch(r"(?:EMP|AP|WO|SITE|D)-[A-Z0-9-]+", value):
                literals.add(value)
        markers = {"seed-data", "SEED_", *literals}
        entries = [list(folder.rglob("*")) for folder in folders]
        files = [file for group in entries for file in group if file.is_file()]
        files.append(server)
        public = root / "public"
        if public.is_symlink():
            result["reason"] = "Symlink public asset directory"
            return result
        if public.is_dir():
            entries.append(list(public.rglob("*")))
            files.extend(file for file in entries[-1] if file.is_file())
        if (not literals or any(not any(file.is_file() for file in group) for group in entries[:len(folders)])
                or any(file.is_symlink() for group in entries for file in group)):
            result["reason"] = "Fixture markers or regular production output missing"
            return result
        manifest = {}
        matches = []
        for file in sorted(files):
            relative = str(file.relative_to(root))
            content = file.read_bytes()
            manifest[relative] = sha(content)
            for marker in markers:
                if (marker in relative or marker.encode() in content
                        or json.dumps(marker, ensure_ascii=True)[1:-1].encode() in content):
                    matches.append({"path": relative, "marker": marker})
        result.update(
            status="failed" if matches else "passed",
            fixture_sha256=sha(source.read_bytes()),
            marker_count=len(markers), artifact_count=len(manifest),
            artifact_manifest_sha256=sha(json.dumps(manifest, sort_keys=True).encode()),
            matches=sorted(matches, key=lambda item: (item["path"], item["marker"])),
        )
    except (OSError, UnicodeError) as error:
        result["reason"] = str(error)
    return result


def production_preflights(commands, root=ROOT):
    """Keep the all-seed claim open; bind bounded artifact proof to this build."""
    general = {
        "id": "no-business-seeds",
        "status": "unreached",
        "scope": "all production routes, assets and business-data paths",
        "reason": "No comprehensive production seed-reachability proof has run",
    }
    known = {
        "id": "known-console-prototype-excluded",
        "status": "unreached",
        "scope": "current build's known Console prototype routes and sampled compiled markers",
        "reason": "Current production build and route/artifact proof have not passed",
    }
    build = next((command for command in commands if command["id"] == "build"), None)
    routes = next((command for command in commands if command["id"] == "production-routes"), None)
    artifacts = {
        "id": "known-business-fixture-artifacts-excluded", "status": "unreached",
        "scope": "known seed-data.ts literals and identifiers in current production route/asset bytes",
        "reason": "Current production build and route proof have not passed",
        "limits": "Bytewise known-marker scan; does not prove other or encoded seeds, backend responses, or database rows",
    }
    if build is None or build["status"] != "passed":
        return [general, known, artifacts]
    if routes is None or routes["status"] not in {"passed", "failed"}:
        return [general, known, artifacts]
    known["status"] = routes["status"]
    known["reason"] = "Known Console prototype exclusion proof failed" if routes["status"] == "failed" else "Known Console prototype exclusion proof passed"
    known["evidence"] = [
        {"command": command["id"], "output": command["output"], "output_sha256": command["output_sha256"]}
        for command in (build, routes)
    ]
    if routes["status"] == "passed":
        artifacts = known_fixture_artifacts(root)
        if artifacts["status"] in {"passed", "failed"}:
            artifacts["evidence"] = known["evidence"]
    return [general, known, artifacts]


def migration_version(path):
    match = re.fullmatch(re.escape(MIGRATION_DIRECTORY) + r"/(\d{4,})_([a-z][a-z0-9_]*)\.sql", path)
    if not match or match[1] != f"{int(match[1]):04d}":
        raise ValueError(f"noncanonical migration path: {path}")
    return int(match[1])


def verify_fork(manifest, actual, delta):
    """Validate immutable originals plus explicitly recorded fork edits/additions."""
    expected = dict(manifest["files"])
    seen = set()
    for entry in delta["backend_source_deltas"]:
        path = entry["path"]
        if path in seen:
            return False
        if path.startswith(MIGRATION_DIRECTORY + "/"):
            try:
                version = migration_version(path)
            except ValueError:
                return False
            if path in expected or entry["before"] is not None or version <= BASE_VERSION:
                return False
        seen.add(path)
        if entry["before"] != expected.get(path) or not entry["reason"]:
            return False
        expected[path] = entry["after"]
    baseline = expected.get(BASELINE_PATH.removeprefix("backend-fork/"))
    if baseline is not None and baseline.get("sha256") != BASELINE_SHA256:
        return False
    try:
        versions = sorted(migration_version(path) for path in expected
                          if path.startswith(MIGRATION_DIRECTORY + "/"))
    except ValueError:
        return False
    suffix = [version for version in versions if version > BASE_VERSION]
    return actual == expected and suffix == list(range(BASE_VERSION + 1, BASE_VERSION + 1 + len(suffix)))


def inspect_fork(root=ROOT):
    """Reuse source custody and expose lineages only after complete verification."""
    manifest_path = root / "backend-fork/SOURCE-CLOSURE.json"
    manifest = json.loads(manifest_path.read_text())
    delta_path = root / "docs/planning/nextjs-v1/fork-delta.json"
    delta = json.loads(delta_path.read_text())
    spec = importlib.util.spec_from_file_location("freeze", root / "tools/freeze-console.py")
    freeze = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(freeze)
    folder = root / "backend-fork" / MIGRATION_DIRECTORY
    census = folder.is_dir()
    if folder.is_symlink():
        raise ValueError("migration directory must not be a symlink")
    if census:
        # The general snapshot filters extensions/cache dirs. Every entry in the
        # migration directory must independently be an admitted regular SQL file.
        for file in folder.iterdir():
            if file.is_symlink():
                raise ValueError(f"symlink migration input: {file.name}")
            try:
                migration_version(MIGRATION_DIRECTORY + "/" + file.name)
            except ValueError:
                census = False
            if not file.is_file():
                census = False
    actual = freeze.snapshot(root / "backend-fork") if census else {}
    valid = (census and actual.get(BASELINE_PATH.removeprefix("backend-fork/"), {}).get("sha256") == BASELINE_SHA256
             and delta["source_manifest_sha256"] == sha(manifest_path.read_bytes())
             and verify_fork(manifest, actual, delta))
    suffix = sorted(migration_version(path) for path in actual
                    if path.startswith(MIGRATION_DIRECTORY + "/")
                    and migration_version(path) > BASE_VERSION) if valid else None
    return {
        "head": manifest["source_git"]["head"], "manifest_sha256": sha(manifest_path.read_bytes()),
        "expected_files": len(manifest["files"]), "discovered_files": len(actual) if census else None,
        "status": "passed" if valid else "failed", "delta_sha256": sha(delta_path.read_bytes()),
        "suffix_versions": suffix,
        "lineages": {"fresh": [0, *suffix], "historical": [*range(1, BASE_VERSION + 1), *suffix]} if valid else None,
        "scope": "immutable frozen manifest plus documented fork deltas; not application acceptance",
    }


def cargo_counts(output, discovery=False):
    if discovery:
        return {"discovered": sum(int(n) for n in re.findall(r"^(\d+) tests?, \d+ benchmarks?$", output, re.M))}
    rows = re.findall(r"^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out;", output, re.M)
    passed, failed, ignored, filtered = [sum(int(row[i]) for row in rows) for i in range(4)]
    return {"executed": passed + failed, "passed": passed, "failed": failed, "skipped": ignored, "filtered": filtered}


def run_command(name, command, directory, evidence, env, timeout=1800):
    output = evidence / f"{name}.log"
    started = time.monotonic()
    result = {"id": name, "command": command, "cwd": str(directory), "discovered": None, "executed": None, "skipped": None}
    try:
        with output.open("wb") as log:
            process = subprocess.run(command, cwd=directory, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
        result.update(status="passed" if process.returncode == 0 else "failed", exit_code=process.returncode)
    except (OSError, subprocess.TimeoutExpired) as error:
        result.update(status="unreached" if isinstance(error, OSError) else "failed", error=str(error))
    result.update(duration_seconds=round(time.monotonic() - started, 3), output=str(output.relative_to(ROOT)), output_sha256=sha(output.read_bytes()))
    print(f"{name}: {result['status']}", flush=True)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", action="store_true", help="Run npm ci/lint/test/build and real Playwright; failures remain failures")
    parser.add_argument("--cargo", action="store_true", help="Discover and run locked Rust unit tests in the independent fork")
    parser.add_argument("--postgres-bin", type=Path, help="Run the real clean-install probe on a temporary PostgreSQL cluster")
    parser.add_argument("--legacy-app", type=Path, help="Also prove the pre-transition binary refuses a fresh baseline")
    parser.add_argument("--baseline-app", type=Path, help="Also prove an installed-baseline upgrade using the preserved pre-suffix binary")
    parser.add_argument("--cargo-integration", action="append", default=[], metavar="PACKAGE:TARGET")
    args = parser.parse_args()
    if args.cargo_integration and not args.postgres_bin:
        parser.error("--cargo-integration requires --postgres-bin for isolated credentials")
    now = datetime.now(timezone.utc)
    evidence = ROOT / ".artifacts/acceptance" / now.strftime("%Y%m%dT%H%M%S.%fZ")
    evidence.mkdir(parents=True)
    plan = (PLANNING / "PLAN.md").read_bytes()
    index = json.loads((PLANNING / "requirements-and-evidence.json").read_text())
    report = {
        "version": 1, "started_at": now.isoformat(), "plan_sha256": sha(plan),
        "release_accepted": False,
        "preflight": preflight(), "commands": [],
        "requirements": [{"id": r["id"], "status": "unreached", "reason": "No executed terminal-outcome acceptance suite is bound", "tests": r["tests"]} for r in index["requirements"]],
        "unreached_suites": ["real-passkey-browser", "direct-protocol-isolation", "two-site-faults", "capacity", "maximum-size-restore", "cutover-rollback", "72-hour-soak", "seven-day-rehearsal"],
        "schema_install": {"status": "unreached", "scope": "Requires the real PostgreSQL probe; historical seed SQL is intentionally preserved for existing ledgers"},
    }
    inputs = [ROOT / p for p in ("package.json", "package-lock.json", "next.config.mjs", "tsconfig.json", "Dockerfile", ".node-version")]
    inputs += [p for folder in ("src", "tests", "e2e", "tools") for p in (ROOT / folder).rglob("*") if p.is_file() and "__pycache__" not in p.parts]
    report["inputs"] = {str(p.relative_to(ROOT)): sha(p.read_bytes()) for p in sorted(inputs)}
    if index["plan_sha256"] != sha(plan):
        raise ValueError("plan/index hash mismatch")
    report["source_closure"] = inspect_fork(ROOT)
    # Prevent tests/build scripts from inheriting live integration credentials.
    env = {k: v for k, v in os.environ.items() if k in {"PATH", "HOME", "TMPDIR", "SHELL", "LANG", "LC_ALL", "RUSTUP_HOME", "CARGO_HOME", "PLAYWRIGHT_PORT"}}
    env.update(CI="1", NEXT_TELEMETRY_DISABLED="1", SQLX_OFFLINE="true", npm_config_cache="/private/tmp/frontend-npm-cache")
    if args.baseline:
        commands = [
            ("npm-ci", ["npm", "ci"]), ("lint", ["npm", "run", "lint"]),
            ("unit", ["npm", "test", "--", "--reporter=json", f"--outputFile={evidence / 'vitest.json'}"]),
            ("build", ["npm", "run", "build"]),
            ("people-prototype-blockers", ["node", "tools/probe-people-prototype-blockers.mjs"]),
            ("production-routes", ["npm", "run", "test:production-routes"]),
            ("storefront-browser-fixture", ["node", "tools/test-storefront-form.mjs"]),
            ("browser-discovery", ["npm", "run", "test:e2e", "--", "--list", "--reporter=json"]),
            ("browser", ["npm", "run", "test:e2e", "--", "--reporter=json"]),
        ]
        env["PLAYWRIGHT_JSON_OUTPUT_FILE"] = str(evidence / "playwright.json")
        for name, command in commands:
            if name == "browser-discovery":
                env["PLAYWRIGHT_JSON_OUTPUT_FILE"] = str(evidence / "playwright-discovery.json")
            elif name == "browser":
                env["PLAYWRIGHT_JSON_OUTPUT_FILE"] = str(evidence / "playwright.json")
            if name == "people-prototype-blockers" and not any(
                item["id"] == "build" and item["status"] == "passed" for item in report["commands"]
            ):
                log = evidence / f"{name}.log"
                log.write_text("unreached: current production build did not pass\n")
                result = {
                    "id": name, "command": command, "cwd": str(ROOT), "status": "unreached",
                    "reason": "Current production build did not pass", "discovered": None,
                    "executed": 0, "skipped": None, "output": str(log.relative_to(ROOT)),
                    "output_sha256": sha(log.read_bytes()),
                }
            else:
                result = run_command(name, command, ROOT, evidence, env)
            if name == "people-prototype-blockers":
                result["scope"] = "Three current People path static blockers only; not authorized SSR acceptance"
                if result["status"] != "unreached":
                    try:
                        payload = json.loads((evidence / f"{name}.log").read_text())
                        if payload["status"] == "unreached" and result.get("exit_code") == 2:
                            result.update(status="unreached", reason=payload["reason"], executed=0)
                        else:
                            checks = payload["checks"]
                            result.update(discovered=len(checks), executed=len(checks),
                                          failed=sum(not check["passed"] for check in checks), skipped=0)
                            if result["status"] != payload["status"]:
                                result.update(status="failed", reason="Probe status and exit code disagree")
                    except (OSError, ValueError, KeyError, TypeError):
                        result.update(status="failed", reason="Probe returned no readable check counts; see command log")
            if name == "unit" and (evidence / "vitest.json").exists():
                tests = json.loads((evidence / "vitest.json").read_text())
                result.update(discovered=tests["numTotalTests"], executed=tests["numPassedTests"] + tests["numFailedTests"], skipped=tests["numPendingTests"], failed=tests["numFailedTests"])
            if name == "browser" and (evidence / "playwright.json").exists():
                stats = json.loads((evidence / "playwright.json").read_text())["stats"]
                result.update(executed=stats["expected"] + stats["unexpected"] + stats["flaky"], skipped=stats["skipped"], failed=stats["unexpected"], flaky=stats["flaky"])
            if name == "browser-discovery" and (evidence / "playwright-discovery.json").exists():
                def count_specs(suites):
                    return sum(sum(len(s["tests"]) for s in suite.get("specs", [])) + count_specs(suite.get("suites", [])) for suite in suites)
                result["discovered"] = count_specs(json.loads((evidence / "playwright-discovery.json").read_text())["suites"])
            report["commands"].append(result)
        report["preflight"][:3] = production_preflights(report["commands"])
    if args.cargo:
        target = str(ROOT / ".artifacts/cargo-target")
        base = ["cargo", "test", "--workspace", "--lib", "--locked", "--offline", "--no-fail-fast", "-j", "2", "--target-dir", target]
        cargo_commands = [("cargo-discovery", base + ["--", "--list"])]
        if not args.postgres_bin:
            cargo_commands.append(("cargo-unit", base))
        for name, command in cargo_commands:
            result = run_command(name, command, ROOT / "backend-fork/backend", evidence, env)
            result.update(cargo_counts((evidence / f"{name}.log").read_text(), discovery=name == "cargo-discovery"))
            report["commands"].append(result)
    app_build_ready = True
    if args.postgres_bin:
        app_build = run_command(
            "cargo-build-app",
            ["cargo", "build", "-p", "console-app", "--locked", "--offline", "--target-dir", str(ROOT / ".artifacts/cargo-target")],
            ROOT / "backend-fork/backend", evidence, env,
        )
        report["commands"].append(app_build)
        app_build_ready = app_build["status"] == "passed"
        if not app_build_ready:
            report["schema_install"]["reason"] = "Cannot probe migrations without a current-source app build"
    if args.postgres_bin and app_build_ready:
        command = ["python3", "tools/check-empty-install.py", "--postgres-bin", str(args.postgres_bin)]
        if args.cargo:
            command.append("--cargo-tests")
        for target in args.cargo_integration:
            command.extend(["--cargo-integration", target])
        if args.legacy_app:
            command.extend(["--legacy-app", str(args.legacy_app)])
        if args.baseline_app:
            command.extend(["--baseline-app", str(args.baseline_app)])
        probe_command = run_command("postgres-clean-install", command, ROOT, evidence, env)
        report["commands"].append(probe_command)
        try:
            summary = json.loads((evidence / "postgres-clean-install.log").read_text())
            probe_path = ROOT / summary["evidence"]
            probe = json.loads(probe_path.read_text())
            probe_result = (probe["status"], probe["executed_checks"], probe["unreached_checks"], summary["counts"])
        except (OSError, ValueError, KeyError, TypeError):
            report["schema_install"]["reason"] = "Clean-install runner produced no readable evidence; see command log"
            if probe_command["status"] == "passed":
                report["schema_install"]["status"] = "failed"
        else:
            report["schema_install"].update(status=probe_result[0], executed=probe_result[1], unreached=probe_result[2])
            report["clean_install_evidence"] = {"path": summary["evidence"], "sha256": sha(probe_path.read_bytes()), "counts": probe_result[3]}
            report["integration_suites"] = probe.get("cargo_suites", [])
            if args.cargo and (probe_path.parent / "cargo-postgres.log").exists():
                result = next(c for c in probe["commands"] if c["name"] == "cargo-postgres")
                report["commands"].append({**result, "id": "cargo-postgres", "status": probe["cargo_status"], **cargo_counts((probe_path.parent / "cargo-postgres.log").read_text())})
    report["finished_at"] = datetime.now(timezone.utc).isoformat()
    (evidence / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"Evidence: {evidence.relative_to(ROOT) / 'report.json'}")
    print(f"Release remains unaccepted: {len(report['requirements'])} module suites unreached.")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
