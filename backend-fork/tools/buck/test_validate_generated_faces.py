#!/usr/bin/env python3
"""Property-style behavior locks for generated-face authority metadata."""

import copy
import importlib.util
import json
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("validate_generated_faces", ROOT / "validate_generated_faces.py")
assert SPEC and SPEC.loader
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)
REGISTRY = json.loads((ROOT / "generated_face_registry.json").read_text(encoding="utf-8"))


def resolved(_root: Path, target: str) -> bool:
    return target.startswith("//tools/buck:generated-face-")


class GeneratedFaceRegistryTests(unittest.TestCase):
    def test_repository_registry_is_valid_with_resolved_writer_targets(self) -> None:
        VALIDATOR.validate_registry(copy.deepcopy(REGISTRY), ROOT.parent.parent, resolved)

    def test_declared_faces_are_precise_and_structured(self) -> None:
        for face in REGISTRY["faces"]:
            self.assertEqual("repo-exec", face["writer"]["kind"])
            self.assertEqual("writer-snapshot", face["drift_gate"]["kind"])
            self.assertIn(face["drift_gate"]["tier"], {"cheap", "expensive"})
            self.assertTrue(all(not path.endswith("/") for path in face["output_patterns"]))
        first_party = next(face for face in REGISTRY["faces"] if face["id"] == "first-party-buck")
        self.assertTrue(all(pattern.endswith("/BUCK") for pattern in first_party["output_patterns"]))

    def test_rejects_overlapping_writable_output_patterns(self) -> None:
        registry = copy.deepcopy(REGISTRY)
        registry["faces"][1]["output_patterns"] = ["backend/crates/**/BUCK"]
        with self.assertRaisesRegex(ValueError, "overlapping writable output roots"):
            VALIDATOR.validate_registry(registry, ROOT.parent.parent, resolved)

    def test_rejects_missing_roots_executables_and_unresolved_targets(self) -> None:
        cases = [
            ("source_roots", ["does-not-exist"]),
            ("output_patterns", ["does-not-exist/**"]),
        ]
        for field, value in cases:
            registry = copy.deepcopy(REGISTRY)
            registry["faces"][0][field] = value
            with self.assertRaises(ValueError, msg=field):
                VALIDATOR.validate_registry(registry, ROOT.parent.parent, resolved)
        registry = copy.deepcopy(REGISTRY)
        registry["faces"][0]["writer"]["executable"] = "tools/buck/nope.sh"
        with self.assertRaisesRegex(ValueError, "executable"):
            VALIDATOR.validate_registry(registry, ROOT.parent.parent, resolved)
        with self.assertRaisesRegex(ValueError, "did not resolve"):
            VALIDATOR.validate_registry(copy.deepcopy(REGISTRY), ROOT.parent.parent, lambda *_: False)

    def test_rejects_raw_or_unknown_gate_and_writer_shapes(self) -> None:
        registry = copy.deepcopy(REGISTRY)
        registry["faces"][0]["drift_gate"] = "npm run arbitrary"
        with self.assertRaisesRegex(ValueError, "drift_gate"):
            VALIDATOR.validate_registry(registry, ROOT.parent.parent, resolved)
        registry = copy.deepcopy(REGISTRY)
        registry["faces"][0]["writer"]["kind"] = "shell"
        with self.assertRaisesRegex(ValueError, "allowlisted"):
            VALIDATOR.validate_registry(registry, ROOT.parent.parent, resolved)

    def test_default_target_resolver_requires_an_exact_target_label(self) -> None:
        target = "//tools/buck:generated-face-first-party"
        with patch.object(
            VALIDATOR.subprocess,
            "run",
            return_value=SimpleNamespace(
                returncode=0,
                stdout=target + "-other\n",
                stderr="",
            ),
        ):
            self.assertEqual(set(), VALIDATOR.default_target_resolver(ROOT.parent.parent, [target]))

        with patch.object(
            VALIDATOR.subprocess,
            "run",
            return_value=SimpleNamespace(
                returncode=0,
                stdout="root" + target + "\n",
                stderr="",
            ),
        ):
            self.assertEqual({target}, VALIDATOR.default_target_resolver(ROOT.parent.parent, [target]))


if __name__ == "__main__":
    unittest.main()
