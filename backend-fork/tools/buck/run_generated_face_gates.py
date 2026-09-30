#!/usr/bin/env python3
"""Run structured generated-face drift gates in an isolated snapshot.

Cheap admission executes only registry faces tagged ``cheap`` and reports every
``expensive`` face as deferred. The full gate executes every registered face.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("generated_face_validator", HERE / "validate_generated_faces.py")
assert SPEC and SPEC.loader
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)

TIERS = frozenset({"cheap", "all"})


def same_face(baseline: Path, snapshot: Path, pattern: str) -> bool:
    root = pattern[:-3] if pattern.endswith("/**") else pattern[:-7] if pattern.endswith("/**/BUCK") else pattern
    left, right = baseline / root, snapshot / root
    if pattern.endswith("/**/BUCK"):
        left_files = {file.relative_to(left) for file in left.rglob("BUCK")}
        right_files = {file.relative_to(right) for file in right.rglob("BUCK")}
        return left_files == right_files and all(
            (left / relative).read_bytes() == (right / relative).read_bytes()
            for relative in left_files
        )
    if pattern.endswith("/**"):
        return shutil.which("diff") is not None and subprocess.run(
            ["diff", "-qr", str(left), str(right)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False
        ).returncode == 0
    return left.read_bytes() == right.read_bytes()


def selected_faces(faces: list[dict[str, object]], tier: str) -> tuple[list[dict[str, object]], list[dict[str, object]]]:
    if tier not in TIERS:
        raise ValueError(f"unsupported generated-face gate tier: {tier}")
    if tier == "all":
        return faces, []
    selected = [face for face in faces if face["drift_gate"]["tier"] == tier]
    return selected, [face for face in faces if face["drift_gate"]["tier"] != tier]


def run(registry_path: Path, baseline: Path, snapshot: Path, *, tier: str = "all") -> int:
    registry = json.loads(registry_path.read_text(encoding="utf-8"))
    # Targets were resolved when validating the candidate. Gate execution only
    # invokes the allowlisted executable paths from the immutable snapshot.
    VALIDATOR.validate_registry(registry, baseline, target_resolver=lambda _root, _target: True)
    faces, deferred = selected_faces(registry["faces"], tier)
    print(f"generated-face-gate: tier={tier} selected={len(faces)} deferred={len(deferred)}")
    for face in deferred:
        print(
            "generated-face-gate: "
            f"{face['id']} ({face['drift_gate']['tier']}) DEFERRED "
            "(run --tier all before merge)"
        )
    for face in faces:
        executable = snapshot / face["writer"]["executable"]
        result = subprocess.run([str(executable)], cwd=snapshot, check=False)
        if result.returncode:
            print(f"generated-face-gate: {face['id']} writer failed", file=sys.stderr)
            return result.returncode
        for pattern in face["output_patterns"]:
            if not same_face(baseline, snapshot, pattern):
                print(f"generated-face-gate: {face['id']} drift at {pattern}", file=sys.stderr)
                return 1
        print(f"generated-face-gate: {face['id']} ({face['drift_gate']['tier']}) PASS")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", required=True, type=Path)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--snapshot", required=True, type=Path)
    parser.add_argument(
        "--tier",
        choices=sorted(TIERS),
        default="all",
        help="cheap runs only cheap faces and reports expensive faces as deferred; all runs every registered face",
    )
    args = parser.parse_args()
    return run(args.registry, args.baseline, args.snapshot, tier=args.tier)


if __name__ == "__main__":
    raise SystemExit(main())
