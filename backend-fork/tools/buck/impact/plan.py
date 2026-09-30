#!/usr/bin/env python3
"""Fail-closed, shadow-only Buck2 impact-manifest planner.

This deliberately does *not* infer dependencies from paths.  Until the pinned
Buck2 Change Detector is vendored as a declared dependency, every non-empty
diff selects the candidate universe.  That is safe to observe and supplies the
same immutable inputs a future BTD adapter will consume, without pretending a
filesystem heuristic is a dependency graph.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
import re
import signal
import shutil
import subprocess
import sys
import tarfile
import uuid
from pathlib import Path
from typing import Any, Iterable


SCHEMA_VERSION = "buck-impact-manifest/v1"
MAX_CHANGED_PATHS = 1024
MAX_TARGETS = 10_000
MAX_RECEIPTS = 32
SHA = re.compile(r"^[0-9a-f]{40}$")


class PlannerError(RuntimeError):
    pass


class PlanningFailure(PlannerError):
    """A typed terminal failure that keeps already-recorded immutable receipts."""

    def __init__(self, message: str, *, base_sha: str | None, candidate_sha: str | None, receipts: list[dict[str, Any]]):
        super().__init__(message)
        self.base_sha = base_sha
        self.candidate_sha = candidate_sha
        self.receipts = receipts


def sha256(value: bytes | str) -> str:
    if isinstance(value, str):
        value = value.encode("utf-8")
    return hashlib.sha256(value).hexdigest()


def bounded_sorted(items: Iterable[str], limit: int) -> tuple[list[str], bool]:
    ordered = sorted(set(items))
    return ordered[:limit], len(ordered) > limit


def normalize_universe(universe: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for item in universe:
        target = item.get("target")
        if not isinstance(target, str) or not target:
            raise PlannerError("Buck target-universe response contains an invalid target")
        labels = item.get("labels", [])
        if not isinstance(labels, list) or not all(isinstance(label, str) for label in labels):
            raise PlannerError(f"Buck target-universe response has invalid labels for {target}")
        result[target] = {"target": target, "labels": sorted(set(labels))}
    return [result[target] for target in sorted(result)]


def target_record(item: dict[str, Any]) -> dict[str, Any]:
    labels = item["labels"]
    return {
        "target": item["target"],
        "owner_labels": [label for label in labels if label.startswith("owner.")],
        "resource_labels": [label for label in labels if label.startswith("resource.")],
        "test_labels": [label for label in labels if label.startswith("test.")],
    }


def build_manifest(
    *,
    base_sha: str,
    candidate_sha: str,
    changed_paths: Iterable[str],
    config_compatible: bool,
    universe: Iterable[dict[str, Any]],
    receipts: list[dict[str, Any]],
    graph_identity: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Build a deterministic manifest without applying an impact heuristic."""
    changed_paths, changed_truncated = bounded_sorted(changed_paths, MAX_CHANGED_PATHS)
    normalized_universe = normalize_universe(universe)
    if len(normalized_universe) > MAX_TARGETS:
        raise PlannerError(
            f"candidate target universe has {len(normalized_universe)} targets; "
            f"refusing bounded full-universe fallback above {MAX_TARGETS}"
        )
    if base_sha == candidate_sha:
        fallback_reason = "identical_revisions"
        impacted: list[dict[str, Any]] = []
    elif not config_compatible:
        fallback_reason = "incompatible_buck_toolchain_or_cell_configuration"
        impacted = [target_record(item) for item in normalized_universe]
    else:
        fallback_reason = "shadow_adapter_full_universe"
        impacted = [target_record(item) for item in normalized_universe]
    return {
        "schema_version": SCHEMA_VERSION,
        "mode": "shadow_only",
        "base_sha": base_sha,
        "candidate_sha": candidate_sha,
        "changed_paths": changed_paths,
        "candidate_target_universe": [target_record(item) for item in normalized_universe],
        "impacted_targets": impacted,
        "fallback_reason": fallback_reason,
        "selection_engine": {
            "name": "buck2_change_detector_adapter",
            "status": "unavailable_unvendored",
            "behavior": "full_universe_fallback",
        },
        "graph_identity": graph_identity or {},
        # The execution order is itself part of an exact command receipt.  The
        # planner's command sequence is fixed, so preserving it is deterministic
        # without obscuring which command was observed first.
        "receipts": receipts[:MAX_RECEIPTS],
        "truncated": {"changed_paths": changed_truncated, "receipts": len(receipts) > MAX_RECEIPTS},
        "build_report_hook": {
            "schema_version": "buck-build-report-hook/v1",
            "activation": "not_active",
            "required_input_fields": ["base_sha", "candidate_sha", "impacted_targets"],
            "buck_argument_template": ["--build-report", "<report-path>"],
        },
    }


def encode_manifest(manifest: dict[str, Any]) -> str:
    return json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"


def failure_manifest(error: PlanningFailure) -> dict[str, Any]:
    """Emit a machine-readable terminal receipt without turning failure into success."""
    return {
        "schema_version": SCHEMA_VERSION,
        "mode": "shadow_only",
        "status": "failed",
        "base_sha": error.base_sha,
        "candidate_sha": error.candidate_sha,
        "failure": {"kind": "planner_error", "message": str(error)},
        "receipts": error.receipts[:MAX_RECEIPTS],
        "truncated": {"receipts": len(error.receipts) > MAX_RECEIPTS},
    }


def run(repo: Path, args: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", "-C", str(repo), *args], cwd=cwd, text=True, capture_output=True, check=False)


def require_clean_repository(repo: Path) -> None:
    probe = run(repo, ["rev-parse", "--is-inside-work-tree"])
    if probe.returncode != 0 or probe.stdout.strip() != "true":
        raise PlannerError("repository is unavailable or is not a Git work tree")
    dirty = run(repo, ["status", "--porcelain=v1", "--untracked-files=all"])
    if dirty.returncode != 0:
        raise PlannerError("could not determine repository cleanliness")
    if dirty.stdout:
        raise PlannerError("repository is dirty; immutable impact planning refuses working-tree input")


def require_commit(repo: Path, revision: str, name: str) -> str:
    if not SHA.fullmatch(revision):
        raise PlannerError(f"{name} must be a full 40-character commit SHA")
    result = run(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"])
    if result.returncode != 0 or result.stdout.strip() != revision:
        raise PlannerError(f"{name} revision is unavailable or not immutable: {revision}")
    return revision


def receipt(
    name: str,
    argv: list[str],
    result: subprocess.CompletedProcess[str],
    *,
    normalized_stdout: str | None = None,
) -> dict[str, Any]:
    value = {
        "name": name,
        "argv": argv,
        "exit_code": result.returncode,
        # `audit cell` contains temporary-worktree absolute paths.  Its receipt
        # must preserve the exact command while hashing its canonical semantics,
        # otherwise equivalent immutable snapshots produce nondeterministic
        # manifests solely because their temporary directory names differ.
        "stdout_sha256": sha256(result.stdout if normalized_stdout is None else normalized_stdout),
        "stderr_sha256": sha256("" if result.returncode == 0 else result.stderr),
        "stderr_policy": "omitted_on_success" if result.returncode == 0 else "retained_on_failure",
    }
    if result.returncode != 0:
        # Planner errors already surface this text; retaining a bounded copy in
        # a receipt keeps asynchronous callers actionable without allowing an
        # unbounded tool error to inflate the manifest.
        value["stderr"] = result.stderr[:4096]
    return value


def archive_receipt(
    revision: str, result: subprocess.CompletedProcess[None], archive: Path
) -> dict[str, Any]:
    """Record an immutable archive without decoding its binary bytes as text."""
    value: dict[str, Any] = {
        "name": f"archive-candidate-{revision[:12]}",
        "argv": ["git", "archive", "--format=tar", revision],
        "exit_code": result.returncode,
        "stderr_sha256": sha256(result.stderr or "") if result.returncode else sha256(""),
        "stderr_policy": "omitted_on_success" if result.returncode == 0 else "retained_on_failure",
    }
    if archive.is_file():
        archive_bytes = archive.read_bytes()
        value["archive_sha256"] = sha256(archive_bytes)
        value["archive_bytes"] = len(archive_bytes)
    if result.returncode != 0:
        value["stderr"] = (result.stderr or "")[:4096]
    return value


def canonical_cell_map(worktree: Path, raw_audit_cell: str) -> dict[str, str]:
    """Turn `buck audit cell` paths into a stable cell -> repo-relative map.

    Buck emits absolute paths.  A cell outside the immutable snapshot cannot be
    compared safely as a repository configuration, so reject it rather than
    accidentally accepting a host-specific mapping.
    """
    # macOS may render the same temporary directory as `/tmp` or `/private/tmp`.
    # Resolve existing parents before calculating the repository-relative path.
    root = worktree.resolve()
    cells: dict[str, str] = {}
    for line in raw_audit_cell.splitlines():
        if not line.strip():
            continue
        name, separator, location = line.partition(": ")
        if not separator or not name or not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
            raise PlannerError(f"unparseable Buck audit cell output: {line!r}")
        if name in cells:
            raise PlannerError(f"duplicate cell in Buck audit output: {name}")
        try:
            relative = Path(location).resolve().relative_to(root)
        except ValueError as error:
            raise PlannerError(f"Buck cell {name} is outside immutable worktree: {location}") from error
        cells[name] = relative.as_posix() if relative.parts else "."
    if not cells:
        raise PlannerError("Buck audit cell produced no cells")
    return dict(sorted(cells.items()))


def git_output(repo: Path, args: list[str], receipts: list[dict[str, Any]], name: str) -> str:
    result = run(repo, args)
    receipts.append(receipt(name, ["git", *args], result))
    if result.returncode != 0:
        raise PlannerError(f"{name} failed: {result.stderr.strip() or result.stdout.strip()}")
    return result.stdout


def configured_local_cells(config: str) -> dict[str, str]:
    """Parse the local `[cells]` mapping without treating missing cells as equal.

    Cell aliases and external cells do not define local configuration roots.
    Values must remain repository-relative; any syntax this tiny parser cannot
    prove safe is terminal rather than silently omitted from compatibility.
    """
    cells: dict[str, str] = {}
    in_cells = False
    for raw_line in config.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            in_cells = line == "[cells]"
            continue
        if not in_cells:
            continue
        name, separator, location = line.partition("=")
        name, location = name.strip(), location.strip()
        if not separator or not re.fullmatch(r"[A-Za-z0-9_.-]+", name) or not location:
            raise PlannerError(f"unparseable local Buck cell declaration: {raw_line!r}")
        path = Path(location)
        if path.is_absolute() or ".." in path.parts:
            raise PlannerError(f"local Buck cell {name} escapes the immutable repository")
        normalized = path.as_posix()
        if normalized == ".":
            normalized = "."
        cells[name] = normalized
    if not cells:
        raise PlannerError("immutable .buckconfig declares no local Buck cells")
    return dict(sorted(cells.items()))


def graph_digest(repo: Path, revision: str, receipts: list[dict[str, Any]]) -> dict[str, Any]:
    listing = git_output(repo, ["ls-tree", "-r", revision], receipts, f"graph-inputs-{revision[:12]}")
    graph_lines = []
    config_lines = []
    objects_by_path: dict[str, str] = {}
    for line in listing.splitlines():
        try:
            metadata, path = line.split("\t", 1)
        except ValueError:
            continue
        fields = metadata.split()
        if len(fields) == 3 and fields[1] == "blob" and re.fullmatch(r"[0-9a-f]{40,64}", fields[2]):
            objects_by_path[path] = fields[2]
        if path in {".buckconfig", "tools/buck2"}:
            config_lines.append(line)
        if Path(path).name == "BUCK" or path.endswith(".bzl") or path in {".buckconfig", "tools/buck2"}:
            graph_lines.append(line)
    config = git_output(repo, ["show", f"{revision}:.buckconfig"], receipts, f"buck-cell-config-{revision[:12]}")
    cells = configured_local_cells(config)
    cell_buck_ids = []
    for cell, location in cells.items():
        buck_path = "BUCK" if location == "." else f"{location}/BUCK"
        cell_buck_ids.append({"cell": cell, "path": buck_path, "object_id": objects_by_path.get(buck_path)})
    manifest = git_output(repo, ["show", f"{revision}:tools/buck2"], receipts, f"pinned-buck-manifest-{revision[:12]}")
    return {
        "revision": revision,
        "configuration_sha256": sha256("\n".join(sorted(config_lines))),
        "configuration_blob_ids": [
            {"path": path, "object_id": objects_by_path[path]} for path in sorted({".buckconfig", "tools/buck2"})
            if path in objects_by_path
        ],
        "configured_cell_buck_ids": cell_buck_ids,
        "target_definition_sha256": sha256("\n".join(sorted(graph_lines))),
        "pinned_buck_manifest_sha256": sha256(manifest),
    }


def changed_paths(repo: Path, base: str, candidate: str, receipts: list[dict[str, Any]]) -> list[str]:
    output = git_output(repo, ["diff", "--name-only", "-z", base, candidate], receipts, "changed-paths")
    return [path for path in output.split("\0") if path]


def parse_universe(raw: str) -> list[dict[str, Any]]:
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as error:
        raise PlannerError(f"Buck uquery did not produce JSON target metadata: {error}") from error
    entries: list[dict[str, Any]] = []
    if isinstance(decoded, dict):
        for target, attributes in decoded.items():
            labels = attributes.get("labels", []) if isinstance(attributes, dict) else []
            entries.append({"target": target, "labels": labels})
    elif isinstance(decoded, list):
        for item in decoded:
            if isinstance(item, str):
                entries.append({"target": item, "labels": []})
            elif isinstance(item, dict):
                target = item.get("target") or item.get("name")
                entries.append({"target": target, "labels": item.get("labels", [])})
    else:
        raise PlannerError("Buck uquery JSON response must be an object or array")
    return normalize_universe(entries)


def buck_probe(worktree: Path, revision: str, receipts: list[dict[str, Any]]) -> tuple[dict[str, str], list[dict[str, Any]]]:
    buck = worktree / "tools/buck2"
    if not buck.is_file():
        raise PlannerError(f"candidate {revision} does not contain the pinned tools/buck2 manifest")
    env = os.environ.copy()
    # The one immutable archive snapshot receives one normal Buck project root.
    # An inherited isolation directory would fragment that probe into extra
    # daemon/cache state and turn the observer into the cold-build source it is
    # measuring.
    env.pop("BUCK_ISOLATION_DIR", None)
    commands = [
        ("buck-audit-cell", [str(buck), "audit", "cell"]),
        ("buck-target-universe", [str(buck), "uquery", "--output-format=json", "--output-attribute=labels", "//..."]),
    ]
    results: list[subprocess.CompletedProcess[str]] = []
    for name, argv in commands:
        result = subprocess.run(argv, cwd=worktree, env=env, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            receipts.append(receipt(name, ["tools/buck2", *argv[1:]], result))
            raise PlannerError(f"{name} failed for {revision}: {result.stderr.strip() or result.stdout.strip()}")
        results.append(result)
    cells = canonical_cell_map(worktree, results[0].stdout)
    receipts.append(
        receipt(
            "buck-audit-cell",
            ["tools/buck2", "audit", "cell"],
            results[0],
            normalized_stdout=encode_manifest(cells),
        )
    )
    receipts.append(receipt("buck-target-universe", ["tools/buck2", *commands[1][1][1:]], results[1]))
    return cells, parse_universe(results[1].stdout)


def ignored_scratch_root(repo: Path) -> Path:
    """Use Buck's watcher-ignored output boundary for all archive snapshots."""
    helper_path = Path(__file__).resolve().parents[1] / "snapshot_root.py"
    spec = importlib.util.spec_from_file_location("buck_snapshot_root", helper_path)
    if spec is None or spec.loader is None:
        raise PlannerError("Buck snapshot-root helper is unavailable")
    helper = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(helper)
        root = helper.root_for(
            repo,
            os.environ.get("CONSOLE_BUCK_IMPACT_SCRATCH_ROOT", "buck-out/buck-impact-snapshots"),
        )
    except (OSError, ValueError) as error:
        raise PlannerError(f"invalid Buck output snapshot root: {error}") from error
    root.mkdir(parents=True, exist_ok=True)
    return root


def extract_archive(archive: Path, destination: Path) -> None:
    """Extract a Git-generated tar only after rejecting escaping member paths."""
    with tarfile.open(archive, "r:") as tar:
        for member in tar.getmembers():
            path = Path(member.name)
            if path.is_absolute() or ".." in path.parts:
                raise PlannerError(f"immutable archive contains unsafe path: {member.name!r}")
            if member.isdev() or member.isfifo() or member.issym() or member.islnk():
                raise PlannerError(f"immutable archive contains unsupported special entry: {member.name!r}")
        tar.extractall(destination)


@contextlib.contextmanager
def candidate_archive_snapshot(repo: Path, candidate: str, receipts: list[dict[str, Any]]) -> Iterable[Path]:
    """Materialize one exact candidate archive and clean it on normal/error/signal exits."""
    root = ignored_scratch_root(repo)
    snapshot = root / f"candidate-{candidate[:12]}-{uuid.uuid4().hex}"
    archive = snapshot / "candidate.tar"
    destination = snapshot / "source"
    previous_handlers: dict[int, Any] = {}

    def terminate(signum: int, _frame: Any) -> None:
        raise PlannerError(f"planner interrupted by signal {signum}")

    try:
        snapshot.mkdir(mode=0o700)
        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.getsignal(signum)
            signal.signal(signum, terminate)
        with archive.open("wb") as output:
            result = subprocess.run(
                ["git", "-C", str(repo), "archive", "--format=tar", candidate],
                stdout=output,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
        receipts.append(archive_receipt(candidate, result, archive))
        if result.returncode != 0:
            raise PlannerError(f"could not archive immutable candidate: {(result.stderr or '').strip()}")
        destination.mkdir(mode=0o700)
        try:
            extract_archive(archive, destination)
        except (OSError, tarfile.TarError) as error:
            raise PlannerError(f"could not extract immutable candidate archive: {error}") from error
        yield destination
    finally:
        for signum, handler in previous_handlers.items():
            signal.signal(signum, handler)
        try:
            if snapshot.exists():
                shutil.rmtree(snapshot)
            receipts.append(
                {
                    "name": f"archive-cleanup-candidate-{candidate[:12]}",
                    "argv": ["cleanup", "<ignored-scratch>", candidate],
                    "exit_code": 0,
                    "stderr_sha256": sha256(""),
                    "stderr_policy": "omitted_on_success",
                }
            )
        except OSError as error:
            receipts.append(
                {
                    "name": f"archive-cleanup-candidate-{candidate[:12]}",
                    "argv": ["cleanup", "<ignored-scratch>", candidate],
                    "exit_code": 1,
                    "stderr_sha256": sha256(str(error)),
                    "stderr_policy": "retained_on_failure",
                    "stderr": str(error)[:4096],
                }
            )
            raise PlannerError(f"could not clean immutable candidate archive: {error}") from error


def plan(repo: Path, base: str, candidate: str) -> dict[str, Any]:
    raw_base, raw_candidate = base, candidate
    receipts: list[dict[str, Any]] = []
    try:
        require_clean_repository(repo)
        base = require_commit(repo, base, "base")
        candidate = require_commit(repo, candidate, "candidate")
        base_identity = graph_digest(repo, base, receipts)
        candidate_identity = graph_digest(repo, candidate, receipts)
        paths = changed_paths(repo, base, candidate, receipts)
        config_compatible = (
            base_identity["configuration_blob_ids"] == candidate_identity["configuration_blob_ids"]
            and base_identity["configured_cell_buck_ids"] == candidate_identity["configured_cell_buck_ids"]
            and base_identity["pinned_buck_manifest_sha256"] == candidate_identity["pinned_buck_manifest_sha256"]
        )

        with candidate_archive_snapshot(repo, candidate, receipts) as candidate_snapshot:
            candidate_cell, universe = buck_probe(candidate_snapshot, candidate, receipts)
        # Configuration identity comes from immutable Git objects.  When the
        # relevant blobs are equal, candidate's immutable cell map is necessarily
        # the base map; when they differ, the conservative full-universe fallback
        # does not need a second Buck daemon merely to prove it is conservative.
        if config_compatible:
            base_cell: dict[str, Any] = candidate_cell
            base_cell_source = "candidate_probe_equal_configuration"
        else:
            base_cell = {"status": "not_probed_incompatible_configuration"}
            base_cell_source = "not_probed_incompatible_configuration"
        return build_manifest(
            base_sha=base,
            candidate_sha=candidate,
            changed_paths=paths,
            config_compatible=config_compatible,
            universe=universe,
            receipts=receipts,
            graph_identity={
                "base": {**base_identity, "cell_map": base_cell, "cell_map_source": base_cell_source},
                "candidate": {**candidate_identity, "cell_map": candidate_cell, "cell_map_source": "candidate_immutable_archive_probe"},
            },
        )
    except PlannerError as error:
        raise PlanningFailure(
            str(error),
            base_sha=base if SHA.fullmatch(base) else raw_base if SHA.fullmatch(raw_base) else None,
            candidate_sha=candidate if SHA.fullmatch(candidate) else raw_candidate if SHA.fullmatch(raw_candidate) else None,
            receipts=receipts,
        ) from error


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--base", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--output", type=Path, help="write deterministic JSON manifest to this path")
    args = parser.parse_args(argv)
    try:
        manifest = plan(args.repo.resolve(), args.base, args.candidate)
        encoded = encode_manifest(manifest)
        if args.output:
            args.output.write_text(encoded, encoding="utf-8")
        else:
            sys.stdout.write(encoded)
        return 0
    except PlanningFailure as error:
        encoded = encode_manifest(failure_manifest(error))
        if args.output:
            args.output.write_text(encoded, encoding="utf-8")
        else:
            sys.stdout.write(encoded)
        print(f"buck-impact-plan: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
