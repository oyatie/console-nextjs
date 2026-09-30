"""Preservation and additive-lineage tests; all generated files are test-only."""
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest

import acceptance
from schema_baseline import BASELINE_PATH


ROOT = Path(__file__).resolve().parents[1]
MIGRATIONS = "backend/crates/platform/db/migrations/"
BASELINE = BASELINE_PATH.removeprefix("backend-fork/")
PINNED_BASELINE = "30fdb9d5cea24ddbded0de37c940dfbeb19478185259c8aa08f81f924b7ea1f1"


def metadata(raw, mode=0o644):
    return {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw), "mode": mode}


class MigrationSuffixTests(unittest.TestCase):
    def setUp(self):
        self.original = MIGRATIONS + "0225_frozen.sql"
        self.manifest = {"files": {self.original: metadata(b"-- historical\n")}}

    def candidate(self, names):
        actual = dict(self.manifest["files"])
        entries = []
        for name in names:
            path = MIGRATIONS + name
            value = metadata(("-- isolated test " + name + "\n").encode())
            actual[path] = value
            entries.append({"path": path, "before": None, "after": value,
                            "reason": "reviewed additive migration"})
        return actual, {"backend_source_deltas": entries}

    def test_empty_and_contiguous_documented_suffixes_are_accepted(self):
        for names in ([], ["0226_auth.sql"], ["0226_auth.sql", "0227_next.sql"]):
            with self.subTest(names=names):
                actual, delta = self.candidate(names)
                self.assertTrue(acceptance.verify_fork(self.manifest, actual, delta))

    def test_historical_edits_deletion_and_rename_stay_forbidden(self):
        changed = metadata(b"-- changed historical\n")
        entry = {"path": self.original, "before": self.manifest["files"][self.original],
                 "after": changed, "reason": "not permission to rewrite applied SQL"}
        self.assertFalse(acceptance.verify_fork(self.manifest, {self.original: changed},
                                               {"backend_source_deltas": [entry]}))
        self.assertFalse(acceptance.verify_fork(self.manifest, {},
                                               {"backend_source_deltas": [{**entry, "after": None}]}))
        actual, delta = self.candidate(["0226_renamed.sql"])
        del actual[self.original]
        self.assertFalse(acceptance.verify_fork(self.manifest, actual, delta))

    def test_old_gapped_duplicate_and_noncanonical_versions_are_rejected(self):
        cases = [
            ["0224_new.sql"], ["0225_new.sql"], ["0227_gap.sql"],
            ["0226_a.sql", "0228_gap.sql"], ["0226_a.sql", "0226_b.sql"],
            ["226_short.sql"], ["00226_padded.sql"], ["0226.sql"],
            ["nested/0226_auth.sql"], ["../migrations/0226_auth.sql"],
            ["0226_auth.up.sql"], ["0226_auth.SQL"], ["0226_auth.txt"],
        ]
        for names in cases:
            with self.subTest(names=names):
                actual, delta = self.candidate(names)
                self.assertFalse(acceptance.verify_fork(self.manifest, actual, delta))

    def test_recorded_suffix_requires_exact_metadata_and_no_extra_files(self):
        actual, delta = self.candidate(["0226_auth.sql"])
        self.assertTrue(acceptance.verify_fork(self.manifest, actual, delta))
        path = MIGRATIONS + "0226_auth.sql"
        for field, value in (("sha256", "0" * 64), ("bytes", 999), ("mode", 0o755)):
            with self.subTest(field=field):
                bad = {**actual, path: {**actual[path], field: value}}
                self.assertFalse(acceptance.verify_fork(self.manifest, bad, delta))
        self.assertFalse(acceptance.verify_fork(self.manifest,
            {**actual, MIGRATIONS + "0227_unrecorded.sql": metadata(b"SELECT 1;")}, delta))
        entry = delta["backend_source_deltas"][0]
        self.assertFalse(acceptance.verify_fork(self.manifest, actual,
            {"backend_source_deltas": [entry, entry]}))
        self.assertFalse(acceptance.verify_fork(self.manifest, actual,
            {"backend_source_deltas": [{**entry, "reason": ""}]}))
        self.assertFalse(acceptance.verify_fork(self.manifest, actual,
            {"backend_source_deltas": [{**entry, "before": metadata(b"invented prior") }]}))

    def test_baseline_pin_cannot_be_replaced_by_a_declared_delta(self):
        raw = (ROOT / BASELINE_PATH).read_bytes()
        self.assertEqual(hashlib.sha256(raw).hexdigest(), PINNED_BASELINE)
        value = metadata(raw)
        entry = {"path": BASELINE, "before": None, "after": value,
                 "reason": "preserved accepted baseline"}
        actual = {**self.manifest["files"], BASELINE: value}
        self.assertTrue(acceptance.verify_fork(self.manifest, actual,
                                               {"backend_source_deltas": [entry]}))
        changed = metadata(raw + b"-- forbidden edit\n")
        self.assertFalse(acceptance.verify_fork(self.manifest,
            {**actual, BASELINE: changed},
            {"backend_source_deltas": [{**entry, "after": changed}]}))
        self.assertFalse(acceptance.verify_fork({"files": actual},
            {**actual, BASELINE: changed},
            {"backend_source_deltas": [{**entry, "before": value, "after": changed}]}))

    def test_harness_discovery_uses_verified_bytes_and_rejects_symlinks(self):
        inspect = getattr(acceptance, "inspect_fork", None)
        self.assertTrue(callable(inspect), "harness needs verified source custody before deriving its lineage")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            fork = root / "backend-fork"
            historic = fork / self.original
            historic.parent.mkdir(parents=True)
            historic.write_bytes(b"-- historical\n")
            historic.chmod(0o644)
            manifest = {**self.manifest, "source_git": {"head": "isolated-fixture"}}
            manifest_path = fork / "SOURCE-CLOSURE.json"
            manifest_path.write_text(json.dumps(manifest))
            delta_dir = root / "docs/planning/nextjs-v1"
            delta_dir.mkdir(parents=True)
            _, delta = self.candidate(["0226_auth.sql", "0227_next.sql"])
            for entry in delta["backend_source_deltas"]:
                file = fork / entry["path"]
                file.write_bytes(("-- isolated test " + file.name + "\n").encode())
                file.chmod(0o644)
            baseline = fork / BASELINE
            baseline.parent.mkdir(parents=True)
            baseline.write_bytes((ROOT / BASELINE_PATH).read_bytes())
            baseline.chmod(0o644)
            baseline_entry = {"path": BASELINE, "before": None,
                              "after": metadata(baseline.read_bytes()),
                              "reason": "preserved accepted baseline"}
            delta["backend_source_deltas"].append(baseline_entry)
            delta["source_manifest_sha256"] = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
            (delta_dir / "fork-delta.json").write_text(json.dumps(delta))
            # inspect_fork uses the actual snapshot implementation, never Console.
            (root / "tools").mkdir()
            shutil.copyfile(ROOT / "tools/freeze-console.py", root / "tools/freeze-console.py")
            result = inspect(root)
            self.assertEqual(result["status"], "passed")
            self.assertEqual(result["suffix_versions"], [226, 227])
            self.assertEqual(result["lineages"],
                             {"fresh": [0, 226, 227], "historical": list(range(1, 228))})
            self.assertEqual(result["manifest_sha256"], delta["source_manifest_sha256"])

            def refused():
                result = inspect(root)
                self.assertEqual(result["status"], "failed")
                self.assertIsNone(result.get("suffix_versions"))
                self.assertIsNone(result.get("lineages"))

            # Baseline0 is a fork addition. Deleting its file AND delta must
            # not turn it into an optional input with usable install lineages.
            baseline.unlink()
            without_baseline = {**delta, "backend_source_deltas":
                                [e for e in delta["backend_source_deltas"] if e != baseline_entry]}
            (delta_dir / "fork-delta.json").write_text(json.dumps(without_baseline))
            refused()
            baseline.write_bytes((ROOT / BASELINE_PATH).read_bytes())
            baseline.chmod(0o644)
            (delta_dir / "fork-delta.json").write_text(json.dumps(delta))
            candidate = fork / MIGRATIONS / "0226_auth.sql"
            saved = candidate.read_bytes()
            candidate.write_bytes(saved + b"-- unrecorded modification\n")
            refused()
            candidate.write_bytes(saved)
            candidate.chmod(0o755)
            refused()
            candidate.chmod(0o644)
            extra = candidate.with_name("0228_unrecorded.sql")
            extra.write_bytes(b"-- isolated unrecorded fixture\n")
            refused()
            extra.unlink()
            # The general source snapshot deliberately ignores some extensions
            # and directories. Migration custody must inventory this whole dir.
            for name in ("0228_unrecorded.SQL", "0228_auth.sql.bak", ".hidden"):
                extra = candidate.with_name(name)
                extra.write_bytes(b"-- isolated unrecorded fixture\n")
                refused()
                extra.unlink()
            nested = candidate.parent / "target"
            nested.mkdir()
            (nested / "0228_nested.sql").write_bytes(b"-- not a migration\n")
            refused()
            shutil.rmtree(nested)
            delta["source_manifest_sha256"] = "0" * 64
            (delta_dir / "fork-delta.json").write_text(json.dumps(delta))
            refused()
            delta["source_manifest_sha256"] = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
            (delta_dir / "fork-delta.json").write_text(json.dumps(delta))
            renamed = candidate.with_name("0226_renamed.sql")
            candidate.rename(renamed)
            refused()
            renamed.rename(candidate)
            self.assertEqual(inspect(root)["lineages"], result["lineages"])
            # Empty suffix is a real valid lineage, distinct from failed census.
            for entry in delta["backend_source_deltas"]:
                if entry != baseline_entry:
                    (fork / entry["path"]).unlink()
            (delta_dir / "fork-delta.json").write_text(json.dumps({
                **delta, "backend_source_deltas": [baseline_entry]}))
            self.assertEqual(inspect(root)["lineages"],
                             {"fresh": [0], "historical": list(range(1, 226))})
            for entry in delta["backend_source_deltas"]:
                if entry == baseline_entry:
                    continue
                file = fork / entry["path"]
                file.write_bytes(("-- isolated test " + file.name + "\n").encode())
                file.chmod(0o644)
            (delta_dir / "fork-delta.json").write_text(json.dumps(delta))
            candidate.unlink()
            outside = root / "outside.sql"
            outside.write_bytes(saved)
            candidate.symlink_to(outside)
            with self.assertRaises(ValueError):
                inspect(root)
            candidate.unlink()
            candidate.write_bytes(saved)
            hidden_link = candidate.with_name("0228_hidden.SQL")
            hidden_link.symlink_to(outside)
            with self.assertRaises(ValueError):
                inspect(root)


if __name__ == "__main__":
    unittest.main()
