import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parent
spec = importlib.util.spec_from_file_location("snapshot_root", ROOT / "snapshot_root.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Tests(unittest.TestCase):
    def test_default_root_is_below_buck_output_boundary(self):
        repo = Path(tempfile.mkdtemp())
        try:
            snapshot = m.create(repo)

            self.assertTrue(snapshot.is_dir())
            self.assertIn((repo / "buck-out").resolve(), snapshot.resolve().parents)
            self.assertNotIn((repo / ".cache").resolve(), snapshot.resolve().parents)

            m.cleanup(repo, snapshot)
            self.assertFalse(snapshot.exists())
        finally:
            shutil.rmtree(repo)

    def test_configured_root_must_remain_below_buck_output_boundary(self):
        repo = Path(tempfile.mkdtemp())
        try:
            configured = repo / "buck-out" / "custom-preflight"
            self.assertEqual(m.root_for(repo, str(configured)), configured.resolve())

            for unsafe in (".cache/buck-preflight", "../escape", str(repo / "buck-out")):
                with self.subTest(unsafe=unsafe):
                    with self.assertRaises(m.SnapshotRootError):
                        m.root_for(repo, unsafe)
        finally:
            shutil.rmtree(repo)


if __name__ == "__main__":
    unittest.main()
