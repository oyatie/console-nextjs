"""Private, exclusive publication for local import artifacts."""

import argparse
from contextlib import contextmanager
import os
from pathlib import Path
import stat
import tempfile


class PrivateArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError("invalid arguments")


@contextmanager
def private_text_output(path: Path):
    # Resolve only the parent so an existing leaf symlink is never followed.
    parent = path.parent.resolve(strict=True)
    for directory in (parent, *parent.parents):
        info = directory.stat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid not in {0, os.geteuid()}:
            raise PermissionError("untrusted output directory")
        if directory == parent and info.st_uid != os.geteuid():
            raise PermissionError("output directory must belong to the operator")
        sticky_ancestor = directory != parent and info.st_uid == 0 and info.st_mode & stat.S_ISVTX
        if info.st_mode & 0o022 and not sticky_ancestor:
            raise PermissionError("writable output directory")
    path = parent / path.name
    # The operator also trusts OS ACLs; staging stays on the publication filesystem.
    with tempfile.TemporaryDirectory(prefix=".private-artifact-", dir=parent) as stage:
        staged = Path(stage) / "content"
        with open(staged, "x", encoding="utf-8",
                  opener=lambda name, flags: os.open(name, flags, 0o600)) as output:
            yield output
            output.flush()
            os.fsync(output.fileno())
        os.link(staged, path)
