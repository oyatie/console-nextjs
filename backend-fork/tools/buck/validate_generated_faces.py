#!/usr/bin/env python3
"""Fail-closed validation for generated-face authority metadata.

The registry is deliberately not a dependency graph. It is a small, structured
allowlist: one repository executable and one resolvable Buck target own each
precise generated output pattern; a structured snapshot gate proves its drift.
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Callable

ID = re.compile(r"^[a-z0-9][a-z0-9-]*$")
FACE_KEYS = {"id", "source_roots", "output_patterns", "writer", "drift_gate"}
WRITER_KEYS = {"kind", "executable", "target"}
GATE_KEYS = {"tier", "kind"}


def fail(message: str) -> None:
    raise ValueError("generated-face-registry: " + message)


def normalized_path(value: object, field: str, *, allow_glob: bool = False) -> str:
    if not isinstance(value, str) or not value:
        fail(f"{field} entries must be non-empty strings")
    if "*" in value and not allow_glob:
        fail(f"{field} may not contain glob syntax")
    if allow_glob and "*" in value and not (value.endswith("/**") or value.endswith("/**/BUCK")):
        fail(f"{field} only permits /** or /**/BUCK globs")
    prefix = value[:-3] if value.endswith("/**") else value[:-7] if value.endswith("/**/BUCK") else value
    path = PurePosixPath(prefix)
    if path.is_absolute() or ".." in path.parts or value.endswith("/"):
        fail(f"{field} has unsafe path {value!r}")
    normalized = str(path)
    if normalized in {".", ""}:
        fail(f"{field} cannot be repository root")
    if value.endswith("/**/BUCK"):
        return normalized + "/**/BUCK"
    return normalized + "/**" if value.endswith("/**") else normalized


def output_roots_overlap(left: str, right: str) -> bool:
    left_root = left[:-3] if left.endswith("/**") else left[:-7] if left.endswith("/**/BUCK") else left
    right_root = right[:-3] if right.endswith("/**") else right[:-7] if right.endswith("/**/BUCK") else right
    return left_root == right_root or left_root.startswith(right_root + "/") or right_root.startswith(left_root + "/")


def default_target_resolver(repo_root: Path, targets: list[str]) -> set[str]:
    """Resolve declared Buck writer targets in one query, never evaluate them."""
    buck = repo_root / "tools" / "buck2"
    if not buck.is_file():
        fail("tools/buck2 is required to resolve writer targets")
    result = subprocess.run(
        [str(buck), "targets", *targets], cwd=repo_root, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if result.returncode:
        return set()
    # `buck2 targets` emits one unconfigured label per line. Normalize only the
    # optional cell prefix (for example `root//pkg:name`), then compare full
    # labels. Substring matching would incorrectly accept `:writer-other`.
    emitted: set[str] = set()
    for line in result.stdout.splitlines():
        label = line.strip().split(maxsplit=1)[0] if line.strip() else ""
        marker = label.find("//")
        if marker >= 0:
            emitted.add(label[marker:])
    return set(targets).intersection(emitted)


def _exists(repo_root: Path, declared: str) -> bool:
    if declared.endswith("/**/BUCK"):
        root = repo_root / declared[:-7]
        return root.is_dir() and any(root.rglob("BUCK"))
    if declared.endswith("/**"):
        root = repo_root / declared[:-3]
        return root.is_dir() and any(root.rglob("*"))
    return (repo_root / declared).exists()


def validate_registry(
    registry: object,
    repo_root: Path | None = None,
    target_resolver: Callable[[Path, str], bool] | None = None,
) -> None:
    repo_root = repo_root or Path.cwd()
    use_default_target_resolver = target_resolver is None
    if not isinstance(registry, dict) or set(registry) != {"schema_version", "faces"}:
        fail("top-level keys must be exactly schema_version and faces")
    if registry["schema_version"] != 2:
        fail("unsupported schema_version")
    faces = registry["faces"]
    if not isinstance(faces, list) or not faces:
        fail("faces must be a non-empty list")

    seen_ids: set[str] = set()
    claimed_outputs: list[tuple[str, str]] = []
    declared_targets: list[tuple[str, str]] = []
    for face in faces:
        if not isinstance(face, dict) or set(face) != FACE_KEYS:
            fail("each face must have exactly " + ", ".join(sorted(FACE_KEYS)))
        face_id = face["id"]
        if not isinstance(face_id, str) or not ID.fullmatch(face_id) or face_id in seen_ids:
            fail(f"face id must be unique lowercase kebab-case: {face_id!r}")
        seen_ids.add(face_id)
        for field, glob in (("source_roots", False), ("output_patterns", True)):
            values = face[field]
            if not isinstance(values, list) or not values:
                fail(f"{face_id}: {field} must be non-empty")
            normalized = [normalized_path(value, f"{face_id}.{field}", allow_glob=glob) for value in values]
            if len(normalized) != len(set(normalized)):
                fail(f"{face_id}: duplicate {field}")
            if not all(_exists(repo_root, value) for value in normalized):
                fail(f"{face_id}: declared {field} must exist in the repository")
            face[field] = normalized
        writer = face["writer"]
        if not isinstance(writer, dict) or set(writer) != WRITER_KEYS:
            fail(f"{face_id}: writer must contain exactly kind, executable, and target")
        if writer["kind"] != "repo-exec":
            fail(f"{face_id}: writer.kind is not allowlisted")
        executable = normalized_path(writer["executable"], f"{face_id}.writer.executable")
        executable_path = repo_root / executable
        if not executable_path.is_file() or not os.access(executable_path, os.X_OK):
            fail(f"{face_id}: writer executable must exist and be executable")
        target = writer["target"]
        if not isinstance(target, str) or not target.startswith("//"):
            fail(f"{face_id}: writer.target must be a Buck target label")
        if not use_default_target_resolver and not target_resolver(repo_root, target):
            fail(f"{face_id}: writer.target did not resolve")
        declared_targets.append((face_id, target))
        gate = face["drift_gate"]
        if not isinstance(gate, dict) or set(gate) != GATE_KEYS:
            fail(f"{face_id}: drift_gate must contain exactly tier and kind")
        if gate["tier"] not in {"cheap", "expensive"} or gate["kind"] != "writer-snapshot":
            fail(f"{face_id}: drift_gate is not allowlisted")
        for output in face["output_patterns"]:
            for existing_id, existing_output in claimed_outputs:
                if output_roots_overlap(output, existing_output):
                    fail(f"overlapping writable output roots: {face_id}:{output} and {existing_id}:{existing_output}")
            claimed_outputs.append((face_id, output))
    if use_default_target_resolver:
        resolved = default_target_resolver(repo_root, [target for _, target in declared_targets])
        for face_id, target in declared_targets:
            if target not in resolved:
                fail(f"{face_id}: writer.target did not resolve")


def main(argv: list[str]) -> int:
    path = Path(argv[1]) if len(argv) == 2 else Path("tools/buck/generated_face_registry.json")
    try:
        registry = json.loads(path.read_text(encoding="utf-8"))
        validate_registry(registry, path.parent.parent.parent)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    print("generated-face-registry: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
