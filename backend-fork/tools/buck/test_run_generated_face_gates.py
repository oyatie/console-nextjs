#!/usr/bin/env python3
"""Execution locks for tiered generated-face gates in immutable snapshots."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("run_generated_face_gates", ROOT / "run_generated_face_gates.py")
assert SPEC and SPEC.loader
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class GeneratedFaceGateRunnerTests(unittest.TestCase):
    def make_tree(self, drift_second: bool = False) -> tuple[Path, Path, Path]:
        temp = Path(tempfile.mkdtemp(prefix="generated-face-gates-"))
        baseline = temp / "base"
        snapshot = temp / "snapshot"
        for root in (baseline, snapshot):
            (root / "src").mkdir(parents=True)
            (root / "out").mkdir()
            (root / "tools").mkdir()
            (root / "src/input.txt").write_text("input\n")
            (root / "out/one.txt").write_text("one\n")
            (root / "out/two.txt").write_text("two\n")
            (root / "out/three.txt").write_text("three\n")
        faces = (("one", "one.txt", "cheap"), ("two", "two.txt", "expensive"), ("three", "three.txt", "cheap"))
        for name, _output, _tier in faces:
            for root in (baseline, snapshot):
                script = root / f"tools/{name}.sh"
                replacement = "printf changed > out/two.txt" if drift_second and name == "two" else ":"
                script.write_text(f"#!/usr/bin/env bash\nprintf {name} >> gate-calls\n{replacement}\n")
                script.chmod(0o755)
        registry = {
            "schema_version": 2,
            "faces": [
                {
                    "id": name,
                    "source_roots": ["src/input.txt"],
                    "output_patterns": [f"out/{output}"],
                    "writer": {"kind": "repo-exec", "executable": f"tools/{name}.sh", "target": f"//tools:{name}"},
                    "drift_gate": {"tier": tier, "kind": "writer-snapshot"},
                }
                for name, output, tier in faces
            ],
        }
        registry_path = baseline / "registry.json"
        registry_path.write_text(json.dumps(registry))
        return temp, baseline, snapshot

    def test_full_gate_runs_every_registered_face(self) -> None:
        temp, baseline, snapshot = self.make_tree()
        try:
            self.assertEqual(0, RUNNER.run(baseline / "registry.json", baseline, snapshot, tier="all"))
            self.assertEqual("onetwothree", (snapshot / "gate-calls").read_text())
        finally:
            shutil.rmtree(temp)

    def test_cheap_gate_runs_only_cheap_faces_and_reports_every_deferral(self) -> None:
        temp, baseline, snapshot = self.make_tree()
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output):
                self.assertEqual(0, RUNNER.run(baseline / "registry.json", baseline, snapshot, tier="cheap"))
            self.assertEqual("onethree", (snapshot / "gate-calls").read_text())
            self.assertIn("tier=cheap selected=2 deferred=1", output.getvalue())
            self.assertIn("two (expensive) DEFERRED", output.getvalue())
            self.assertIn("run --tier all before merge", output.getvalue())
        finally:
            shutil.rmtree(temp)

    def test_full_gate_fails_closed_on_expensive_face_drift(self) -> None:
        temp, baseline, snapshot = self.make_tree(drift_second=True)
        try:
            self.assertEqual(1, RUNNER.run(baseline / "registry.json", baseline, snapshot, tier="all"))
            self.assertEqual("onetwo", (snapshot / "gate-calls").read_text())
        finally:
            shutil.rmtree(temp)

    def test_full_gate_catches_every_kotlin_root_artifact(self) -> None:
        # The Kotlin generator replaces the complete client tree except for
        # handwritten src/test. These root files are generated too, so each
        # must independently fail a full snapshot closure when regenerated.
        artifacts = (
            "clients/kotlin/.openapi-generator-ignore",
            "clients/kotlin/README.md",
            "clients/kotlin/proguard-rules.pro",
        )
        for artifact in artifacts:
            with self.subTest(artifact=artifact):
                temp = Path(tempfile.mkdtemp(prefix="generated-face-kotlin-"))
                baseline, snapshot = temp / "base", temp / "snapshot"
                try:
                    for root in (baseline, snapshot):
                        (root / "src").mkdir(parents=True)
                        (root / "tools").mkdir()
                        (root / artifact).parent.mkdir(parents=True, exist_ok=True)
                        (root / "src/input.txt").write_text("input\n")
                        (root / artifact).write_text("generated\n")
                    script = snapshot / "tools/kotlin.sh"
                    script.write_text(f"#!/usr/bin/env bash\nprintf changed > {artifact}\n")
                    script.chmod(0o755)
                    (baseline / "tools/kotlin.sh").write_text("#!/usr/bin/env bash\n:")
                    (baseline / "tools/kotlin.sh").chmod(0o755)
                    registry = {
                        "schema_version": 2,
                        "faces": [{
                            "id": "openapi-kotlin",
                            "source_roots": ["src/input.txt"],
                            "output_patterns": [artifact],
                            "writer": {"kind": "repo-exec", "executable": "tools/kotlin.sh", "target": "//tools:kotlin"},
                            "drift_gate": {"tier": "expensive", "kind": "writer-snapshot"},
                        }],
                    }
                    registry_path = baseline / "registry.json"
                    registry_path.write_text(json.dumps(registry))
                    self.assertEqual(1, RUNNER.run(registry_path, baseline, snapshot, tier="all"))
                finally:
                    shutil.rmtree(temp)

    def test_rejects_unknown_tier(self) -> None:
        temp, baseline, snapshot = self.make_tree()
        try:
            with self.assertRaisesRegex(ValueError, "unsupported generated-face gate tier"):
                RUNNER.run(baseline / "registry.json", baseline, snapshot, tier="surprise")
        finally:
            shutil.rmtree(temp)


if __name__ == "__main__":
    unittest.main()
