#!/usr/bin/env python3
"""Behavior locks for the immutable, shadow-only impact planner."""

from __future__ import annotations

import importlib.util
import json
import hashlib
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
PLAN = ROOT / "tools/buck/impact/plan.py"

GIT_FIXTURE_DEFAULTS = {
    "GIT_ASKPASS": "/bin/false",
    "GIT_CONFIG_COUNT": "0",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_SYSTEM": "/dev/null",
    "GIT_NO_REPLACE_OBJECTS": "1",
    "GIT_TERMINAL_PROMPT": "0",
    "LC_ALL": "C",
}


def git_fixture_environment(source_environment=None, overrides=None):
    source = os.environ if source_environment is None else source_environment
    environment = {
        name: value for name, value in source.items() if not name.startswith("GIT_")
    }
    environment.update(GIT_FIXTURE_DEFAULTS)
    environment.update(overrides or {})
    return environment


def fixture_run(args, *, env=None, git_overrides=None, **kwargs):
    return subprocess.run(
        args,
        env=git_fixture_environment(env, git_overrides),
        **kwargs,
    )


def load_plan_module():
    spec = importlib.util.spec_from_file_location("impact_plan", PLAN)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ImpactPlannerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.module = load_plan_module()

    def test_identical_revisions_need_no_targets(self) -> None:
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="a" * 40,
            changed_paths=[],
            config_compatible=True,
            universe=[{"target": "root//backend:unit", "labels": ["owner.backend", "resource.none"]}],
            receipts=[],
        )
        self.assertEqual([], manifest["impacted_targets"])
        self.assertEqual("identical_revisions", manifest["fallback_reason"])

    def test_unknown_path_falls_back_to_sorted_full_universe(self) -> None:
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="b" * 40,
            changed_paths=["unowned-file.txt"],
            config_compatible=True,
            universe=[
                {"target": "root//z:target", "labels": ["resource.none", "owner.z"]},
                {"target": "root//a:target", "labels": ["owner.a", "resource.postgres"]},
            ],
            receipts=[],
        )
        self.assertEqual("shadow_adapter_full_universe", manifest["fallback_reason"])
        self.assertEqual(["root//a:target", "root//z:target"], [item["target"] for item in manifest["impacted_targets"]])
        self.assertEqual(["owner.a"], manifest["impacted_targets"][0]["owner_labels"])
        self.assertEqual(["resource.postgres"], manifest["impacted_targets"][0]["resource_labels"])

    def test_configuration_change_is_never_a_selective_plan(self) -> None:
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="b" * 40,
            changed_paths=[".buckconfig"],
            config_compatible=False,
            universe=[{"target": "root//backend:unit", "labels": []}],
            receipts=[],
        )
        self.assertEqual("incompatible_buck_toolchain_or_cell_configuration", manifest["fallback_reason"])
        self.assertEqual(["root//backend:unit"], [item["target"] for item in manifest["impacted_targets"]])

    def test_distinct_worktree_paths_with_identical_cell_maps_are_compatible(self) -> None:
        first = self.module.canonical_cell_map(
            Path("/tmp/base-worktree"),
            "root: /tmp/base-worktree\nprelude: /tmp/base-worktree/prelude\ntoolchains: /tmp/base-worktree/toolchains\n",
        )
        second = self.module.canonical_cell_map(
            Path("/tmp/candidate-worktree"),
            "root: /tmp/candidate-worktree\nprelude: /tmp/candidate-worktree/prelude\ntoolchains: /tmp/candidate-worktree/toolchains\n",
        )
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="b" * 40,
            changed_paths=["backend/BUCK"],
            config_compatible=first == second,
            universe=[{"target": "root//backend:unit", "labels": []}],
            receipts=[],
        )
        self.assertEqual(first, second)
        self.assertEqual("shadow_adapter_full_universe", manifest["fallback_reason"])

    def test_changed_cell_mapping_is_incompatible(self) -> None:
        base = self.module.canonical_cell_map(
            Path("/tmp/base-worktree"), "root: /tmp/base-worktree\nprelude: /tmp/base-worktree/prelude\n"
        )
        candidate = self.module.canonical_cell_map(
            Path("/tmp/candidate-worktree"),
            "root: /tmp/candidate-worktree\nprelude: /tmp/candidate-worktree/alternate-prelude\n",
        )
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="b" * 40,
            changed_paths=[".buckconfig"],
            config_compatible=base == candidate,
            universe=[{"target": "root//backend:unit", "labels": []}],
            receipts=[],
        )
        self.assertNotEqual(base, candidate)
        self.assertEqual("incompatible_buck_toolchain_or_cell_configuration", manifest["fallback_reason"])

    def test_two_identical_sha_plans_are_byte_identical(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            (repo / "tools").mkdir(parents=True)
            (repo / ".gitignore").write_text(".tmp/\n", encoding="utf-8")
            (repo / ".buckconfig").write_text("[cells]\n  root = .\n", encoding="utf-8")
            buck = repo / "tools/buck2"
            buck.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                "case \"$1\" in\n"
                "  audit) printf 'root: %s\\nprelude: %s/prelude\\n' \"$PWD\" \"$PWD\" ;;\n"
                "  uquery) printf '%s\\n' '{\"root//backend:unit\":{\"labels\":[\"owner.backend\",\"resource.none\"]}}' ;;\n"
                "  *) exit 2 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            buck.chmod(0o755)
            for command in (
                ["git", "init", "-q", str(repo)],
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                ["git", "-C", str(repo), "config", "user.name", "Impact Test"],
                ["git", "-C", str(repo), "add", "."],
                ["git", "-C", str(repo), "commit", "-qm", "fixture"],
            ):
                fixture_run(command, check=True)
            sha = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            first, second = Path(directory) / "first.json", Path(directory) / "second.json"
            for output in (first, second):
                fixture_run(
                    ["python3", str(PLAN), "--repo", str(repo), "--base", sha, "--candidate", sha, "--output", str(output)],
                    text=True,
                    capture_output=True,
                    check=True,
                )
            first_bytes, second_bytes = first.read_bytes(), second.read_bytes()
            self.assertEqual(first_bytes, second_bytes)
            self.assertEqual(hashlib.sha256(first_bytes).hexdigest(), hashlib.sha256(second_bytes).hexdigest())

    def test_manifest_json_is_bounded_and_stably_serialized(self) -> None:
        manifest = self.module.build_manifest(
            base_sha="a" * 40,
            candidate_sha="b" * 40,
            changed_paths=[f"path-{index:04d}" for index in range(2000, -1, -1)],
            config_compatible=True,
            universe=[{"target": "root//b:t", "labels": []}, {"target": "root//a:t", "labels": []}],
            receipts=[],
        )
        encoded_once = self.module.encode_manifest(manifest)
        encoded_twice = self.module.encode_manifest(manifest)
        self.assertEqual(encoded_once, encoded_twice)
        decoded = json.loads(encoded_once)
        self.assertEqual(1024, len(decoded["changed_paths"]))
        self.assertTrue(decoded["truncated"]["changed_paths"])
        self.assertEqual(sorted(decoded["changed_paths"]), decoded["changed_paths"])

    def test_non_sha_revision_is_rejected_before_git_or_buck_access(self) -> None:
        with self.assertRaisesRegex(self.module.PlannerError, "full 40-character commit SHA"):
            self.module.require_commit(Path("/does/not/need/to/exist"), "main", "base")

    def test_nonempty_diff_uses_only_a_candidate_archive_probe_without_registered_worktrees(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            (repo / "tools").mkdir(parents=True)
            (repo / ".gitignore").write_text(".tmp/\n", encoding="utf-8")
            (repo / ".buckconfig").write_text("[cells]\n  root = .\n", encoding="utf-8")
            buck = repo / "tools/buck2"
            buck.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                "printf '%s|%s|%s\\n' \"$PWD\" \"$1\" \"${BUCK_ISOLATION_DIR-unset}\" >> \"$BUCK_IMPACT_TEST_LOG\"\n"
                "case \"$1\" in\n"
                "  audit) printf 'root: %s\\n' \"$PWD\" ;;\n"
                "  uquery) printf '%s\\n' '{\"root//backend:unit\":{\"labels\":[\"owner.backend\"]}}' ;;\n"
                "  *) exit 2 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            buck.chmod(0o755)
            for command in (
                ["git", "init", "-q", str(repo)],
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                ["git", "-C", str(repo), "config", "user.name", "Impact Test"],
                ["git", "-C", str(repo), "add", "."],
                ["git", "-C", str(repo), "commit", "-qm", "base"],
            ):
                fixture_run(command, check=True)
            base = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            (repo / "changed.txt").write_text("candidate\n", encoding="utf-8")
            fixture_run(["git", "-C", str(repo), "add", "changed.txt"], check=True)
            fixture_run(["git", "-C", str(repo), "commit", "-qm", "candidate"], check=True)
            candidate = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            before = fixture_run(
                ["git", "-C", str(repo), "worktree", "list", "--porcelain"], text=True, capture_output=True, check=True
            ).stdout
            log = Path(directory) / "buck.log"
            completed = fixture_run(
                ["python3", str(PLAN), "--repo", str(repo), "--base", base, "--candidate", candidate],
                text=True,
                capture_output=True,
                env={**os.environ, "BUCK_IMPACT_TEST_LOG": str(log)},
                check=True,
            )
            after = fixture_run(
                ["git", "-C", str(repo), "worktree", "list", "--porcelain"], text=True, capture_output=True, check=True
            ).stdout
            manifest = json.loads(completed.stdout)
            self.assertEqual(before, after)
            self.assertFalse(any("worktree" in receipt["argv"] for receipt in manifest["receipts"]))
            self.assertEqual(2, len(log.read_text(encoding="utf-8").splitlines()))
            self.assertTrue(all(line.endswith("|unset") for line in log.read_text(encoding="utf-8").splitlines()))
            self.assertEqual("shadow_adapter_full_universe", manifest["fallback_reason"])
            self.assertEqual(["root//backend:unit"], [entry["target"] for entry in manifest["impacted_targets"]])
            snapshot_root = repo / "buck-out" / "buck-impact-snapshots"
            self.assertFalse(list(snapshot_root.glob("candidate-*")))
            self.assertTrue(
                all(str(snapshot_root) in line for line in log.read_text(encoding="utf-8").splitlines())
            )
            self.assertFalse(any(receipt["name"] == "configuration-compatibility" for receipt in manifest["receipts"]))
            self.assertEqual(
                manifest["graph_identity"]["base"]["configuration_blob_ids"],
                manifest["graph_identity"]["candidate"]["configuration_blob_ids"],
            )
            self.assertEqual(
                manifest["graph_identity"]["base"]["pinned_buck_manifest_sha256"],
                manifest["graph_identity"]["candidate"]["pinned_buck_manifest_sha256"],
            )

    def test_incompatible_configuration_falls_back_without_a_base_buck_probe(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            (repo / "tools").mkdir(parents=True)
            (repo / ".gitignore").write_text(".tmp/\n", encoding="utf-8")
            (repo / ".buckconfig").write_text("[cells]\n  root = .\n", encoding="utf-8")
            buck = repo / "tools/buck2"
            buck.write_text(
                "#!/usr/bin/env bash\n"
                "set -euo pipefail\n"
                "printf '%s|%s\\n' \"$PWD\" \"$1\" >> \"$BUCK_IMPACT_TEST_LOG\"\n"
                "case \"$1\" in\n"
                "  audit) printf 'root: %s\\n' \"$PWD\" ;;\n"
                "  uquery) printf '%s\\n' '[\"root//backend:unit\"]' ;;\n"
                "  *) exit 2 ;;\n"
                "esac\n",
                encoding="utf-8",
            )
            buck.chmod(0o755)
            for command in (
                ["git", "init", "-q", str(repo)],
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                ["git", "-C", str(repo), "config", "user.name", "Impact Test"],
                ["git", "-C", str(repo), "add", "."],
                ["git", "-C", str(repo), "commit", "-qm", "base"],
            ):
                fixture_run(command, check=True)
            base = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            (repo / ".buckconfig").write_text("[cells]\n  root = alternate\n", encoding="utf-8")
            fixture_run(["git", "-C", str(repo), "add", ".buckconfig"], check=True)
            fixture_run(["git", "-C", str(repo), "commit", "-qm", "candidate"], check=True)
            candidate = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            log = Path(directory) / "buck.log"
            completed = fixture_run(
                ["python3", str(PLAN), "--repo", str(repo), "--base", base, "--candidate", candidate],
                text=True,
                capture_output=True,
                env={**os.environ, "BUCK_IMPACT_TEST_LOG": str(log)},
                check=True,
            )
            manifest = json.loads(completed.stdout)
            self.assertEqual("incompatible_buck_toolchain_or_cell_configuration", manifest["fallback_reason"])
            self.assertEqual(2, len(log.read_text(encoding="utf-8").splitlines()))

    def test_declared_cell_buck_file_change_is_configuration_incompatible(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            (repo / "tools").mkdir(parents=True)
            (repo / ".buckconfig").write_text("[cells]\n  root = .\n  none = none\n", encoding="utf-8")
            buck = repo / "tools/buck2"
            buck.write_text(
                "#!/usr/bin/env bash\ncase \"$1\" in audit) printf 'root: %s\\n' \"$PWD\" ;; uquery) echo '[\"root//:unit\"]' ;; esac\n",
                encoding="utf-8",
            )
            buck.chmod(0o755)
            for command in (
                ["git", "init", "-q", str(repo)],
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                ["git", "-C", str(repo), "config", "user.name", "Impact Test"],
                ["git", "-C", str(repo), "add", "."],
                ["git", "-C", str(repo), "commit", "-qm", "base"],
            ):
                fixture_run(command, check=True)
            base = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            (repo / "none").mkdir()
            (repo / "none" / "BUCK").write_text("filegroup(name = 'new-cell')\n", encoding="utf-8")
            fixture_run(["git", "-C", str(repo), "add", "none/BUCK"], check=True)
            fixture_run(["git", "-C", str(repo), "commit", "-qm", "declare cell build file"], check=True)
            candidate = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            completed = fixture_run(
                ["python3", str(PLAN), "--repo", str(repo), "--base", base, "--candidate", candidate],
                text=True,
                capture_output=True,
                check=True,
            )
            manifest = json.loads(completed.stdout)
            self.assertEqual("incompatible_buck_toolchain_or_cell_configuration", manifest["fallback_reason"])
            self.assertNotEqual(
                manifest["graph_identity"]["base"]["configured_cell_buck_ids"],
                manifest["graph_identity"]["candidate"]["configured_cell_buck_ids"],
            )

    def test_failed_probe_emits_typed_failure_manifest_with_cleanup_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo = Path(directory) / "repo"
            (repo / "tools").mkdir(parents=True)
            (repo / ".buckconfig").write_text("[cells]\n  root = .\n", encoding="utf-8")
            buck = repo / "tools/buck2"
            buck.write_text("#!/usr/bin/env bash\necho audit failed >&2\nexit 17\n", encoding="utf-8")
            buck.chmod(0o755)
            for command in (
                ["git", "init", "-q", str(repo)],
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                ["git", "-C", str(repo), "config", "user.name", "Impact Test"],
                ["git", "-C", str(repo), "add", "."],
                ["git", "-C", str(repo), "commit", "-qm", "fixture"],
            ):
                fixture_run(command, check=True)
            sha = fixture_run(
                ["git", "-C", str(repo), "rev-parse", "HEAD"], text=True, capture_output=True, check=True
            ).stdout.strip()
            output = Path(directory) / "failure.json"
            completed = fixture_run(
                ["python3", str(PLAN), "--repo", str(repo), "--base", sha, "--candidate", sha, "--output", str(output)],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(1, completed.returncode)
            self.assertEqual("", completed.stdout)
            manifest = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual("failed", manifest["status"])
            self.assertEqual("planner_error", manifest["failure"]["kind"])
            self.assertTrue(any(receipt["name"] == "buck-audit-cell" and receipt["exit_code"] == 17 for receipt in manifest["receipts"]))
            self.assertTrue(any(receipt["name"].startswith("archive-cleanup-candidate-") for receipt in manifest["receipts"]))

    def test_archive_rejects_symbolic_and_hard_link_entries(self) -> None:
        import tarfile

        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "destination"
            cases = ((tarfile.SYMTYPE, "../../outside"), (tarfile.SYMTYPE, "inside"), (tarfile.LNKTYPE, "inside"), (tarfile.FIFOTYPE, ""))
            for index, (entry_type, linkname) in enumerate(cases):
                archive = Path(directory) / f"unsafe-{index}.tar"
                with tarfile.open(archive, "w") as tar:
                    entry = tarfile.TarInfo("entry")
                    entry.type, entry.linkname = entry_type, linkname
                    tar.addfile(entry)
                with self.assertRaisesRegex(self.module.PlannerError, "unsupported special entry"):
                    self.module.extract_archive(archive, destination)


if __name__ == "__main__":
    unittest.main()
