import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("freeze", Path(__file__).with_name("freeze-console.py"))
freeze = importlib.util.module_from_spec(spec)
spec.loader.exec_module(freeze)


class SourceClosureTests(unittest.TestCase):
    def test_copy_preserves_bytes_and_modes_excludes_runtime_and_cannot_overwrite(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "source"
            target = Path(temp) / "fork"
            (source / "backend/target").mkdir(parents=True)
            file = source / "backend/lib.rs"
            file.write_text("// source including uncommitted edits\n")
            file.chmod(0o755)
            (source / "backend/target/cache.json").write_text("{}")
            (source / "backend/.env").write_text("NOT_A_REAL_SECRET=fixture")
            with patch.object(freeze, "git_state", return_value={"head": "fixture"}):
                manifest = freeze.freeze(source, target)
            self.assertEqual(manifest["file_count"], 1)
            self.assertEqual(file.read_bytes(), (target / "backend/lib.rs").read_bytes())
            self.assertEqual((target / "backend/lib.rs").stat().st_mode & 0o777, 0o755)
            self.assertFalse((target / "backend/target").exists())
            self.assertFalse((target / "backend/.env").exists())
            with self.assertRaises(ValueError):
                freeze.freeze(source, target)

    def test_symlinks_and_credential_material_fail_closed(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp)
            (source / "backend").mkdir()
            file = source / "backend/lib.rs"
            file.symlink_to(source / "outside.rs")
            with self.assertRaises(ValueError):
                freeze.snapshot(source)
            file.unlink()
            file.write_text("-----BEGIN PRIVATE KEY-----\nfixture")
            with self.assertRaises(ValueError):
                freeze.snapshot(source)

    def test_drift_retries_and_never_publishes_incoherent_source(self):
        with tempfile.TemporaryDirectory() as temp:
            source = Path(temp) / "source"
            (source / "backend").mkdir(parents=True)
            (source / "backend/lib.rs").write_text("// fixture\n")
            target = Path(temp) / "fork"
            states = [{"head": str(i)} for i in range(6)]
            with patch.object(freeze, "git_state", side_effect=states) as read:
                with self.assertRaises(RuntimeError):
                    freeze.freeze(source, target)
                self.assertEqual(read.call_count, 6)
            self.assertFalse(target.exists())
            self.assertEqual(list(Path(temp).glob(".console-freeze-*")), [])


if __name__ == "__main__":
    unittest.main()
