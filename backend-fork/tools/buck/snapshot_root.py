#!/usr/bin/env python3
"""Create and clean archive snapshots below Buck's ignored output boundary."""
from __future__ import annotations
import argparse, os, shutil, sys, tempfile
from pathlib import Path

class SnapshotRootError(ValueError): pass

def root_for(repo: Path, configured: str | None = None) -> Path:
    repo, buck_out = repo.resolve(), (repo / "buck-out").resolve()
    value = configured or "buck-out/buck-preflight-snapshots"
    candidate = Path(value)
    candidate = (repo / candidate).resolve() if not candidate.is_absolute() else candidate.resolve()
    if buck_out not in candidate.parents:
        raise SnapshotRootError("snapshot root must stay below the Buck-ignored buck-out directory")
    return candidate

def create(repo: Path, configured: str | None = None) -> Path:
    root = root_for(repo, configured)
    root.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix='snapshot-', dir=root))

def cleanup(repo: Path, path: Path, configured: str | None = None) -> None:
    root, target = root_for(repo, configured), path.resolve()
    if root not in target.parents or target.name == root.name:
        raise SnapshotRootError('refusing to clean a path outside the snapshot root')
    shutil.rmtree(target)

def main() -> int:
    p=argparse.ArgumentParser(); p.add_argument('--repo',required=True,type=Path); p.add_argument('--cleanup',type=Path); args=p.parse_args()
    try:
        if args.cleanup: cleanup(args.repo,args.cleanup,os.environ.get('CONSOLE_BUCK_PREFLIGHT_SCRATCH_ROOT'))
        else: print(create(args.repo,os.environ.get('CONSOLE_BUCK_PREFLIGHT_SCRATCH_ROOT')))
    except (OSError, SnapshotRootError) as e: print(f'buck-preflight-snapshot: {e}',file=sys.stderr); return 1
    return 0
if __name__=='__main__': raise SystemExit(main())
