#!/usr/bin/env python3
"""Copy an allowlisted working tree without writing to the source repository."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
from datetime import datetime, timezone


ROOTS = (
    "backend", "third-party/rust/vendor", "scripts", "tools", "registry",
    "ops", "docs/current", "docs/specs", "docs/decisions", "docs/program",
    "security", ".config", "deploy",
)
FILES = (
    "README.md", "AGENTS.md", "package.json", "package-lock.json",
    "docs/reference/일일업무진행현황_0605.xlsx",
    "docs/reference/업무일지_26.05.27.xlsx",
    "backend/crates/payroll/ui/pkg/console_payroll_ui_bg.wasm",
    "backend/app/tests/fixtures/attendance-offset.xlsx",
    "third-party/rust/vendor/umya-spreadsheet-3.0.0-quickxml41/LICENSE",
)
EXCLUDED_DIRS = {"target", "node_modules", ".git", "__pycache__", "buck-out"}
SOURCE_EXTENSIONS = {
    ".rs", ".toml", ".lock", ".sql", ".json", ".yaml", ".yml", ".md",
    ".mjs", ".js", ".cjs", ".ts", ".py", ".sh", ".bzl", ".txt", ".version", ".tf", ".tofu",
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(source, *args):
    return subprocess.check_output(
        ["git", "--no-optional-locks", "-C", str(source), *args],
        env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"},
    )


def paths(source):
    selected = set()
    for root in ROOTS:
        if (source / root).is_symlink():
            raise ValueError(f"symlink source root: {root}")
        for base, dirs, files in os.walk(source / root, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in EXCLUDED_DIRS)
            for name in dirs:
                if (Path(base) / name).is_symlink():
                    raise ValueError(f"symlink directory: {Path(base) / name}")
            for name in files:
                p = Path(base) / name
                if p.suffix in SOURCE_EXTENSIONS and not name.startswith(".env"):
                    selected.add(p.relative_to(source).as_posix())
    selected.update(name for name in FILES if (source / name).exists())
    return sorted(selected)


def read_file(source, name):
    path = source / name
    # Check every parent: no symlink can escape the source closure.
    for part in (path, *path.parents):
        if part == source:
            break
        if part.is_symlink():
            raise ValueError(f"symlink is not a source file: {name}")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"not a regular file: {name}")
        data = stream.read()
    # Never copy credential material accidentally introduced into source files.
    if any(marker in data for marker in (
        b"-----BEGIN PRIVATE KEY-----", b"-----BEGIN RSA PRIVATE KEY-----",
        b"-----BEGIN OPENSSH PRIVATE KEY-----", b"-----BEGIN EC PRIVATE KEY-----",
    )):
        raise ValueError(f"private-key marker requires source review: {name}")
    return data, stat.S_IMODE(metadata.st_mode)


def snapshot(source):
    files = {}
    for name in paths(source):
        data, mode = read_file(source, name)
        files[name] = {"sha256": digest(data), "bytes": len(data), "mode": mode}
    return files


def git_state(source):
    head = git(source, "rev-parse", "HEAD").decode().strip()
    index = {}
    for entry in git(source, "ls-files", "--stage", "-z").split(b"\0"):
        if not entry:
            continue
        meta, name = entry.split(b"\t", 1)
        mode, oid, stage = meta.decode().split()
        if stage != "0":
            raise ValueError("source has unresolved index conflicts")
        index[name.decode()] = {"mode": mode, "blob": oid}
    tree = {}
    for entry in git(source, "ls-tree", "-r", "-z", head).split(b"\0"):
        if entry:
            meta, name = entry.split(b"\t", 1)
            mode, kind, oid = meta.decode().split()
            tree[name.decode()] = {"mode": mode, "blob": oid, "kind": kind}
    # Preserve deletions too; only metadata about allowlisted paths is retained.
    def selected(name):
        return name in FILES or any(name.startswith(p + "/") for p in ROOTS)
    return {
        "head": head,
        "index": {k: v for k, v in index.items() if selected(k)},
        "head_tree": {k: v for k, v in tree.items() if selected(k)},
    }


def freeze(source, destination):
    source = source.resolve(strict=True)
    destination = destination.absolute()
    if destination.exists() or destination.is_symlink():
        raise ValueError("destination already exists; refusing to overwrite a fork")
    if source == destination or source in destination.parents or destination in source.parents:
        raise ValueError("source and destination must be independent trees")
    destination.parent.mkdir(parents=True, exist_ok=True)
    for attempt in range(1, 4):
        before_git = git_state(source)
        before = snapshot(source)
        with tempfile.TemporaryDirectory(prefix=".console-freeze-", dir=destination.parent) as temp:
            staged = Path(temp) / "tree"
            staged.mkdir()
            for name, expected in before.items():
                data, mode = read_file(source, name)
                if digest(data) != expected["sha256"] or mode != expected["mode"]:
                    break
                target = staged / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
                target.chmod(mode)
            else:
                after = snapshot(source)
                after_git = git_state(source)
                if before == after and before_git == after_git and snapshot(staged) == before:
                    manifest = {
                        "version": 1, "source": str(source),
                        "captured_at": datetime.now(timezone.utc).isoformat(),
                        "attempt": attempt, "source_git": before_git,
                        "allowlisted_roots": ROOTS, "allowlisted_files": FILES,
                        "excluded_directories": sorted(EXCLUDED_DIRS),
                        "file_count": len(before),
                        "files": before,
                        "verification": "working bytes, file set, modes, HEAD and index stable before/after copy; destination rehashed",
                        "status": "source_snapshot_only_not_release_evidence",
                    }
                    (staged / "SOURCE-CLOSURE.json").write_text(
                        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
                    )
                    staged.rename(destination)
                    return manifest
    raise RuntimeError("source changed during all three copy attempts; no fork published")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    result = freeze(args.source, args.destination)
    print(json.dumps({"head": result["source_git"]["head"], "files": result["file_count"], "attempt": result["attempt"]}))
