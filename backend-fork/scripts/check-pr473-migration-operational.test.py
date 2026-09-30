#!/usr/bin/env python3

from __future__ import annotations

import copy
import importlib.util
import io
import json
import os
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("check-pr473-migration-operational.py")
SPEC = importlib.util.spec_from_file_location("pr473_gate", SCRIPT)
assert SPEC and SPEC.loader
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def valid_manifest() -> dict:
    return json.loads(
        (SCRIPT.parents[1] / gate.MANIFEST_PATH).read_text(encoding="utf-8")
    )


class ManifestTests(unittest.TestCase):
    def test_accepts_canonical_manifest(self) -> None:
        gate.validate_manifest(valid_manifest())

    def test_rejects_wrong_typed_release_authority(self) -> None:
        for key, value in (
            ("release_phase", "contract"),
            ("deployment_authorized", True),
            ("command_only_claim_authorized", True),
        ):
            with self.subTest(key=key):
                manifest = valid_manifest()
                manifest[key] = value
                with self.assertRaises(gate.GateError):
                    gate.validate_manifest(manifest)

    def test_rejects_duplicate_or_missing_guarded_tests(self) -> None:
        duplicate = valid_manifest()
        duplicate["guarded_tests"][-1] = copy.deepcopy(duplicate["guarded_tests"][0])
        with self.assertRaises(gate.GateError):
            gate.validate_manifest(duplicate)

        missing = valid_manifest()
        missing["guarded_tests"].pop()
        with self.assertRaises(gate.GateError):
            gate.validate_manifest(missing)

    def test_rejects_an_unapproved_but_unique_test_tuple(self) -> None:
        manifest = valid_manifest()
        manifest["guarded_tests"][0]["name"] = "invented_unique_test"
        with self.assertRaises(gate.GateError):
            gate.validate_manifest(manifest)

    def test_rejects_noncanonical_json(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "gate.json"
            path.write_text(json.dumps(valid_manifest()), encoding="utf-8")
            with self.assertRaises(gate.GateError):
                gate.load_manifest(path)


class CommandLineTests(unittest.TestCase):
    def test_backend_directory_resolves_to_repository_root(self) -> None:
        backend = SCRIPT.parents[1] / "backend"
        self.assertEqual(gate.resolve_repo_root(None, backend), SCRIPT.parents[1])


class ExecutionTests(unittest.TestCase):
    @staticmethod
    def successful_output(name: str) -> str:
        # The shape libtest actually emits, ` finished in` tail included.  A
        # hand-written literal that stopped at `filtered out;` made this gate
        # unfalsifiable: the summary pattern could not match any real run.
        return (
            "running 1 test\n"
            f"test {name} ... ok\n"
            "\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.06s\n"
        )

    @staticmethod
    def buck_console_replay(name: str) -> str:
        """Byte-shape of how Buck2's non-TTY console replays a passing test."""
        stamp = "[2026-07-25T19:38:11.116+00:00]"
        lines = (
            "✓ Pass: root//tools/buck:pr473-ontology-key-revision-postgres (2.1s)",
            "---- STDOUT ----",
            "",
            "running 1 test",
            f"test {name} ... ok",
            "",
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 2.06s",
            "",
            "---- STDERR ----",
        )
        return "".join(f"{stamp} {line}\n" if line else f"{stamp}\n" for line in lines)

    @staticmethod
    def completed_runs() -> list[subprocess.CompletedProcess[str]]:
        specs = ExecutionTests.specs()
        return [
            subprocess.CompletedProcess(["buck"], 0, "", "")
            for _ in gate.OPERATIONAL_SQLX_TARGETS[2:]
        ] + [
            subprocess.CompletedProcess(["buck"], 0, ExecutionTests.successful_output(name), "")
            for _, name in specs
        ]

    @staticmethod
    def specs() -> tuple[tuple[str, str], ...]:
        return tuple(
            (
                gate.TARGET_BY_SOURCE_AND_BINARY[(test["source"], test["target"])],
                test["name"],
            )
            for test in valid_manifest()["guarded_tests"]
        )

    @staticmethod
    def metadata() -> dict[str, dict]:
        rows: dict[str, dict] = {}
        for test in valid_manifest()["guarded_tests"]:
            key = (test["source"], test["target"])
            wrapper = gate.TARGET_BY_SOURCE_AND_BINARY[key]
            rust = gate.RUST_TARGET_BY_SOURCE_AND_BINARY[key]
            rows[gate.target_name(rust)] = {
                "name": gate.target_name(rust),
                "crate_root": test["source"],
                "mapped_srcs": {f"root//{test['source']}": test["source"]},
            }
            rows[gate.target_name(wrapper)] = {
                "name": gate.target_name(wrapper),
                "args": [f"$(location root{rust})"],
                "deps": [f"root{rust}"],
            }
        return rows

    def test_requires_the_buck_owned_disposable_postgres_harness(self) -> None:
        completed = self.completed_runs()

        with patch.object(gate, "resolved_target_metadata", return_value=self.metadata()), patch.object(gate, "run", side_effect=completed) as run_mock:
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(gate.execute(SCRIPT.parents[1]), 0)

        self.assertEqual(run_mock.call_count, len(completed))
        for target, call in zip(gate.OPERATIONAL_SQLX_TARGETS[2:], run_mock.call_args_list):
            command = call.args[0]
            self.assertEqual(
                command,
                [
                    str(SCRIPT.parents[1] / gate.BUCK_POSTGRES_HARNESS),
                    target,
                    "--test-executor-stdout=-",
                ],
            )
            self.assertNotIn("cargo", command)
            self.assertNotIn("DATABASE_URL", call.kwargs["env"])
        specs = self.specs()
        for (target, name), call in zip(specs, run_mock.call_args_list[2:]):
            self.assertEqual(
                call.args[0],
                [str(SCRIPT.parents[1] / gate.BUCK_POSTGRES_HARNESS), target, "--test-executor-stdout=-"],
            )
            self.assertEqual(call.kwargs["env"]["CONSOLE_BUCK_NEEDS_POSTGRES_TEST_EXACT"], name)

    def test_rejects_reusable_ci_database_urls_before_harness_invocation(self) -> None:
        completed = self.completed_runs()
        inherited = {
            "DATABASE_URL": "postgres://superuser@ci/reusable",
            "CONSOLE_APALIS_OWNER_DATABASE_URL": "postgres://owner@ci/reusable",
            "CONSOLE_APALIS_RUNTIME_DATABASE_URL": "postgres://runtime@ci/reusable",
            "CONSOLE_APALIS_ADMIN_DATABASE_URL": "postgres://admin@ci/reusable",
        }
        with patch.dict(os.environ, inherited, clear=False):
            with patch.object(gate, "resolved_target_metadata", return_value=self.metadata()), patch.object(gate, "run", side_effect=completed) as run_mock:
                with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    self.assertEqual(gate.execute(SCRIPT.parents[1]), 0)
        environment = run_mock.call_args.kwargs["env"]
        for key in inherited:
            self.assertNotIn(key, environment)

    def test_accepts_timestamp_prefixed_buck_receipts_on_stderr(self) -> None:
        """Keep accepting the compact stderr replay emitted by the CI console."""
        prefix = "[2026-07-25T21:38:32.088+00:00] "
        completed = [
            subprocess.CompletedProcess(["buck"], 0, "", "")
            for _ in gate.OPERATIONAL_SQLX_TARGETS[2:]
        ] + [
            subprocess.CompletedProcess(
                ["buck"],
                0,
                "",
                "".join(
                    prefix + line
                    for line in self.successful_output(name).splitlines(keepends=True)
                ),
            )
            for _, name in self.specs()
        ] + [subprocess.CompletedProcess(["cargo"], 0, "", "")]

        with patch.object(
            gate, "resolved_target_metadata", return_value=self.metadata()
        ), patch.object(gate, "run", side_effect=completed):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(gate.execute(SCRIPT.parents[1]), 0)

    def test_fails_when_buck_harness_fails(self) -> None:
        completed = subprocess.CompletedProcess(["buck"], 19, "", "")
        with patch.object(gate, "run", return_value=completed):
            with self.assertRaisesRegex(gate.GateError, "Buck2 disposable PostgreSQL"):
                gate.execute(SCRIPT.parents[1])

    def test_accepts_receipts_replayed_by_the_buck_console_on_stderr(self) -> None:
        """CI's real delivery path: stdout empty, receipts on Buck2's stderr."""
        specs = self.specs()
        completed = [
            subprocess.CompletedProcess(["buck"], 0, "", "")
            for _ in gate.OPERATIONAL_SQLX_TARGETS[2:]
        ] + [
            subprocess.CompletedProcess(["buck"], 0, "", self.buck_console_replay(name))
            for _, name in specs
        ]
        with patch.object(gate, "resolved_target_metadata", return_value=self.metadata()), patch.object(gate, "run", side_effect=completed):
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(gate.execute(SCRIPT.parents[1]), 0)

    def test_rejects_a_non_timestamp_line_prefix(self) -> None:
        """The tolerated prefix is a timestamp, not arbitrary leading text."""
        name = valid_manifest()["guarded_tests"][0]["name"]
        spoofed = self.buck_console_replay(name).replace("[2026-07-25T19:38:11.116+00:00]", "[log]")
        with self.assertRaisesRegex(gate.GateError, "one exact libtest"):
            gate.assert_exact_rust_test_result(spoofed, name, "//tools/buck:test")

    def test_rejects_a_bare_spoofed_test_line(self) -> None:
        name = valid_manifest()["guarded_tests"][0]["name"]
        with self.assertRaisesRegex(gate.GateError, "one exact libtest"):
            gate.assert_exact_rust_test_result(f"test {name} ... ok\n", name, "//tools/buck:test")

    def test_rejects_zero_or_multiple_selected_tests(self) -> None:
        name = valid_manifest()["guarded_tests"][0]["name"]
        for output in (
            "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;\n",
            self.successful_output(name) + self.successful_output(name),
        ):
            with self.subTest(output=output):
                with self.assertRaisesRegex(gate.GateError, "one exact libtest"):
                    gate.assert_exact_rust_test_result(output, name, "//tools/buck:test")

    def test_rejects_drifted_generated_rust_declaration(self) -> None:
        target = gate.RUST_TARGET_BY_SOURCE_AND_BINARY[
            ("backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs", "key_revision_migration_upgrade")
        ]
        metadata = self.metadata()
        metadata[gate.target_name(target)]["crate_root"] = "wrong.rs"
        with patch.object(gate, "resolved_target_metadata", return_value=metadata):
            with self.assertRaisesRegex(gate.GateError, "no longer binds"):
                gate.guarded_test_specs(SCRIPT.parents[1], valid_manifest())

    def test_rejects_drifted_postgres_wrapper_declaration(self) -> None:
        target = gate.TARGET_BY_SOURCE_AND_BINARY[
            ("backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs", "key_revision_migration_upgrade")
        ]
        metadata = self.metadata()
        metadata[gate.target_name(target)]["deps"] = ["root//wrong:target"]
        with patch.object(gate, "resolved_target_metadata", return_value=metadata):
            with self.assertRaisesRegex(gate.GateError, "no longer executes"):
                gate.guarded_test_specs(SCRIPT.parents[1], valid_manifest())

    def test_rejects_comment_only_source_spoof_in_resolved_metadata(self) -> None:
        target = gate.RUST_TARGET_BY_SOURCE_AND_BINARY[
            ("backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs", "key_revision_migration_upgrade")
        ]
        metadata = self.metadata()
        metadata[gate.target_name(target)]["mapped_srcs"] = {"comment": "backend/crates/ontology/adapter-postgres/tests/key_revision_migration_upgrade.rs"}
        with patch.object(gate, "resolved_target_metadata", return_value=metadata):
            with self.assertRaisesRegex(gate.GateError, "no longer binds"):
                gate.guarded_test_specs(SCRIPT.parents[1], valid_manifest())

    def test_accepts_reordered_resolved_metadata(self) -> None:
        metadata = dict(reversed(list(self.metadata().items())))
        with patch.object(gate, "resolved_target_metadata", return_value=metadata):
            self.assertEqual(gate.guarded_test_specs(SCRIPT.parents[1], valid_manifest()), self.specs())

    def test_rejects_wrong_or_duplicate_metadata_names(self) -> None:
        target = "//tools/buck:pr473-ontology-key-revision-postgres"
        rows = [{"name": "wrong"}, {"name": "wrong"}]
        completed = subprocess.CompletedProcess(["buck"], 0, json.dumps(rows), "")
        with patch.object(subprocess, "run", return_value=completed):
            with self.assertRaisesRegex(gate.GateError, "exactly once"):
                gate.resolved_target_metadata(SCRIPT.parents[1], (target, target.replace("ontology", "leave")))


if __name__ == "__main__":
    unittest.main()
