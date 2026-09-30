import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("acceptance", Path(__file__).with_name("acceptance.py"))
acceptance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acceptance)


class AcceptanceTests(unittest.TestCase):
    def test_failed_clean_install_without_summary_still_writes_report(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            planning = root / "docs/planning/nextjs-v1"
            planning.mkdir(parents=True)
            (planning / "PLAN.md").write_text("plan")
            (planning / "requirements-and-evidence.json").write_text(json.dumps({
                "plan_sha256": acceptance.sha(b"plan"), "requirements": [],
            }))
            for name in ("package.json", "package-lock.json", "next.config.ts", "tsconfig.json", "Dockerfile", ".node-version"):
                (root / name).write_text("")

            def failed_probe(name, command, directory, evidence, env):
                (evidence / f"{name}.log").write_text("missing PostgreSQL binary\n")
                return {"id": name, "status": "failed", "output": str(evidence / f"{name}.log"),
                        "output_sha256": acceptance.sha(b"missing PostgreSQL binary\n")}

            with (patch.object(acceptance, "ROOT", root), patch.object(acceptance, "PLANNING", planning),
                  patch.object(acceptance, "preflight", return_value=[]),
                  patch.object(acceptance, "inspect_fork", return_value={"status": "passed"}),
                  patch.object(acceptance, "run_command", side_effect=failed_probe),
                  patch("sys.argv", ["acceptance.py", "--postgres-bin", "/missing"]),
                  patch("builtins.print")):
                self.assertEqual(acceptance.main(), 1)

            reports = list((root / ".artifacts/acceptance").glob("*/report.json"))
            self.assertEqual(len(reports), 1)
            report = json.loads(reports[0].read_text())
            self.assertEqual([command["id"] for command in report["commands"]], ["cargo-build-app"])
            self.assertEqual(report["commands"][0]["status"], "failed")
            self.assertEqual(report["schema_install"]["status"], "unreached")
            self.assertIn("current-source app build", report["schema_install"]["reason"])

    def test_fork_deltas_do_not_hide_unrecorded_edits_or_historical_migrations(self):
        manifest = {"files": {"app.rs": {"sha256": "old"}}}
        actual = {"app.rs": {"sha256": "new"}}
        entry = {"path": "app.rs", "before": {"sha256": "old"}, "after": {"sha256": "new"}, "reason": "install lineage"}
        delta = {"backend_source_deltas": [entry]}
        self.assertTrue(acceptance.verify_fork(manifest, actual, delta))
        self.assertFalse(acceptance.verify_fork(manifest, {**actual, "extra.rs": {}}, delta))
        self.assertFalse(acceptance.verify_fork(manifest, actual, {"backend_source_deltas": [entry, entry]}))
        forbidden = {**entry, "path": "backend/crates/platform/db/migrations/0021.sql", "before": None}
        self.assertFalse(acceptance.verify_fork(manifest, actual, {"backend_source_deltas": [forbidden]}))

    def test_cargo_counts_distinguish_discovered_executed_and_ignored(self):
        self.assertEqual(acceptance.cargo_counts("7 tests, 0 benchmarks\n1 test, 0 benchmarks\n", True), {"discovered": 8})
        result = acceptance.cargo_counts("test result: FAILED. 3 passed; 1 failed; 2 ignored; 0 measured; 2 filtered out; finished in 0.0s\n")
        self.assertEqual(result, {"executed": 4, "passed": 3, "failed": 1, "skipped": 2, "filtered": 2})

    def test_prototype_seed_is_unreached_without_production_build_proof(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.assertTrue(all(p["status"] == "unreached" for p in acceptance.preflight(root)))
            (root / "src/lib").mkdir(parents=True)
            (root / "src/lib/store.ts").write_text('import { SEED } from "./seed-data";\n')
            result = {p["id"]: p for p in acceptance.preflight(root)}
            self.assertEqual(result["no-business-seeds"]["status"], "unreached")
            self.assertEqual(result["no-plaintext-business-storage"]["status"], "passed")
            (root / "src/lib/store.ts").write_text("localStorage.setItem('business', '{}');\n")
            result = {p["id"]: p for p in acceptance.preflight(root)}
            self.assertEqual(result["no-plaintext-business-storage"]["status"], "failed")
            self.assertEqual(result["no-plaintext-business-storage"]["lines"], [1])

    def test_known_console_proof_requires_same_run_build_and_route_checks(self):
        build = {"id": "build", "status": "passed", "output": "build.log", "output_sha256": "build-hash"}
        routes = {"id": "production-routes", "status": "passed", "output": "routes.log", "output_sha256": "routes-hash"}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for commands in ([], [routes], [{**build, "status": "failed"}, routes], [build], [build, {**routes, "status": "unreached"}]):
                self.assertEqual([p["status"] for p in acceptance.production_preflights(commands, root)], ["unreached"] * 3)
            failed = {p["id"]: p for p in acceptance.production_preflights([build, {**routes, "status": "failed"}], root)}
            self.assertEqual(failed["no-business-seeds"]["status"], "unreached")
            self.assertEqual(failed["known-console-prototype-excluded"]["status"], "failed")
            self.assertEqual(failed["known-business-fixture-artifacts-excluded"]["status"], "unreached")
            passed = {p["id"]: p for p in acceptance.production_preflights([build, routes], root)}
            self.assertEqual(passed["no-business-seeds"]["status"], "unreached")
            self.assertEqual(passed["known-console-prototype-excluded"]["status"], "passed")
            self.assertEqual(passed["known-business-fixture-artifacts-excluded"]["status"], "unreached")
            self.assertEqual(
                passed["known-console-prototype-excluded"]["evidence"],
                [
                    {"command": "build", "output": "build.log", "output_sha256": "build-hash"},
                    {"command": "production-routes", "output": "routes.log", "output_sha256": "routes-hash"},
                ],
            )

    def test_planted_public_storefront_fixture_is_not_cleared_by_known_console_proof(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "src/lib").mkdir(parents=True)
            (root / "src/lib/store.ts").write_text('import { SEED } from "./seed-data";\n')
            (root / "src/app/storefront").mkdir(parents=True)
            (root / "src/app/storefront/page.public.tsx").write_text('const ACTIVE_LISTING = { name: "fabricated business fact" };\n')
            checks = acceptance.preflight(root)
            checks[:3] = acceptance.production_preflights([
                {"id": "build", "status": "passed", "output": "build.log", "output_sha256": "build-hash"},
                {"id": "production-routes", "status": "passed", "output": "routes.log", "output_sha256": "routes-hash"},
            ], root)
            statuses = {p["id"]: p["status"] for p in checks}
            self.assertEqual(statuses["known-console-prototype-excluded"], "passed")
            self.assertEqual(statuses["no-business-seeds"], "unreached")
            self.assertEqual(list(statuses.values()).count("passed"), 2)
            self.assertEqual(list(statuses.values()).count("unreached"), 6)

    def test_known_fixture_probe_fails_when_production_asset_contains_seed_value(self):
        build = {"id": "build", "status": "passed", "output": "build.log", "output_sha256": "build-hash"}
        routes = {"id": "production-routes", "status": "passed", "output": "routes.log", "output_sha256": "routes-hash"}
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "src/lib/seed-data.ts"
            source.parent.mkdir(parents=True)
            source.write_text('export const SEED_LISTING = { name: "Fixture Business 123" };\n')
            for folder in (".next/static", ".next/server", ".next/standalone/.next/server"):
                (root / folder).mkdir(parents=True)
            asset = root / ".next/static/storefront.js"
            asset.write_text('console.log("live data only");')
            (root / ".next/server/page.js").write_text("module.exports = {};")
            (root / ".next/standalone/.next/server/page.js").write_text("module.exports = {};")
            (root / ".next/standalone/server.js").write_text("// server")
            probe = {p["id"]: p for p in acceptance.production_preflights([build, routes], root)}
            self.assertEqual(probe["known-business-fixture-artifacts-excluded"]["status"], "passed")
            self.assertEqual(probe["no-business-seeds"]["status"], "unreached")

            asset.write_text('console.log("Fixture Business 123");')
            probe = {p["id"]: p for p in acceptance.production_preflights([build, routes], root)}
            self.assertEqual(probe["known-business-fixture-artifacts-excluded"]["status"], "failed")
            self.assertEqual(probe["known-business-fixture-artifacts-excluded"]["matches"], [
                {"path": ".next/static/storefront.js", "marker": "Fixture Business 123"},
            ])
            self.assertEqual(probe["no-business-seeds"]["status"], "unreached")

    def test_baseline_records_people_probe_only_after_a_passed_build(self):
        for build_passed in (False, True):
            with self.subTest(build_passed=build_passed), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                planning = root / "docs/planning/nextjs-v1"
                planning.mkdir(parents=True)
                (planning / "PLAN.md").write_text("plan")
                (planning / "requirements-and-evidence.json").write_text(json.dumps({
                    "plan_sha256": acceptance.sha(b"plan"), "requirements": [],
                }))
                for name in ("package.json", "package-lock.json", "next.config.ts", "tsconfig.json", "Dockerfile", ".node-version"):
                    (root / name).write_text("")
                called = []

                def fake_command(name, command, directory, evidence, env):
                    called.append(name)
                    failed = name == "people-prototype-blockers" or (name == "build" and not build_passed)
                    output = json.dumps({"status": "failed", "checks": [{"passed": False}] * 3}) if name == "people-prototype-blockers" else "ok\n"
                    log = evidence / f"{name}.log"
                    log.write_text(output)
                    return {"id": name, "status": "failed" if failed else "passed", "exit_code": int(failed),
                            "output": str(log.relative_to(root)), "output_sha256": acceptance.sha(log.read_bytes())}

                with (patch.object(acceptance, "ROOT", root), patch.object(acceptance, "PLANNING", planning),
                      patch.object(acceptance, "preflight", return_value=[]),
                      patch.object(acceptance, "inspect_fork", return_value={"status": "passed"}),
                      patch.object(acceptance, "run_command", side_effect=fake_command),
                      patch("sys.argv", ["acceptance.py", "--baseline"]), patch("builtins.print")):
                    self.assertEqual(acceptance.main(), 1)

                report = json.loads(next((root / ".artifacts/acceptance").glob("*/report.json")).read_text())
                probe = next(item for item in report["commands"] if item["id"] == "people-prototype-blockers")
                self.assertEqual(probe["status"], "failed" if build_passed else "unreached")
                self.assertEqual(probe["executed"], 3 if build_passed else 0)
                self.assertIn("not authorized SSR acceptance", probe["scope"])
                self.assertEqual("people-prototype-blockers" in called, build_passed)
                self.assertFalse(report["release_accepted"])
                preflight = {item["id"]: item["status"] for item in report["preflight"]}
                self.assertEqual(preflight["known-console-prototype-excluded"], "passed" if build_passed else "unreached")
                self.assertEqual(preflight["no-business-seeds"], "unreached")

if __name__ == "__main__":
    unittest.main()
