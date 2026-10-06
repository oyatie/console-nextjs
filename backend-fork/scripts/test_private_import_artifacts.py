"""Synthetic, local-only privacy regressions for the two import CLIs.

Run with the hash-pinned importer test dependencies.
No live workbook, database, or destination is used.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

from openpyxl import Workbook


SCRIPTS = Path(__file__).resolve().parent
COSS = SCRIPTS / "import_coss_group_workbooks.py"
EQUIPMENT = SCRIPTS / "import_equipment_master_list.py"
SENTINEL = "SYNTHETIC_PRIVATE_SENTINEL_7e921"
ORG_SLUGS = (
    "coss", "cnl", "dsl", "lso", "knl", "cheongun-hr",
    "cheongun-logis", "jy-tech",
)


def run_cli(script: Path, *args: str) -> subprocess.CompletedProcess[str]:
    old_umask = os.umask(0)
    try:
        return subprocess.run(
            [sys.executable, str(script), *map(str, args)],
            text=True,
            capture_output=True,
            check=False,
            timeout=30,
            env={**os.environ, "PYTHONNOUSERSITE": "1"},
        )
    finally:
        os.umask(old_umask)


def mode(path: Path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


class PrivateTextOutputTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_stage_is_private_before_first_byte_and_fsynced_before_link(self) -> None:
        from private_artifact_output import private_text_output

        target = self.root / "out.sql"
        real_fsync = os.fsync
        real_link = os.link
        fsync_seen = False

        def checked_fsync(fd: int) -> None:
            nonlocal fsync_seen
            fsync_seen = True
            real_fsync(fd)

        def checked_link(source: str, destination: str, *args: object, **kwargs: object) -> None:
            self.assertTrue(fsync_seen, "content must be fsynced before publication")
            real_link(source, destination, *args, **kwargs)

        old_umask = os.umask(0)
        try:
            with patch("os.fsync", checked_fsync), patch("os.link", checked_link):
                with private_text_output(target) as out:
                    stages = [entry for entry in self.root.iterdir() if entry.is_dir()]
                    self.assertEqual(len(stages), 1)
                    self.assertEqual(mode(stages[0]), 0o700)
                    staged_files = list(stages[0].iterdir())
                    self.assertEqual(len(staged_files), 1)
                    self.assertEqual(mode(staged_files[0]), 0o600)
                    self.assertFalse(target.exists())
                    out.write("synthetic private content\n")
        finally:
            os.umask(old_umask)

        self.assertTrue(fsync_seen)
        self.assertEqual(mode(target), 0o600)
        self.assertEqual(target.read_text(encoding="utf-8"), "synthetic private content\n")
        self.assertEqual(list(self.root.iterdir()), [target])

    def test_existing_file_and_symlink_cannot_be_replaced(self) -> None:
        from private_artifact_output import private_text_output

        existing = self.root / "existing.sql"
        existing.write_text("original", encoding="utf-8")
        with self.assertRaises(FileExistsError):
            with private_text_output(existing) as out:
                out.write("replacement")
        self.assertEqual(existing.read_text(encoding="utf-8"), "original")

        destination = self.root / "destination.sql"
        destination.write_text("outside", encoding="utf-8")
        symlink = self.root / "symlink.sql"
        symlink.symlink_to(destination)
        with self.assertRaises(FileExistsError):
            with private_text_output(symlink) as out:
                out.write("replacement")
        self.assertTrue(symlink.is_symlink())
        self.assertEqual(destination.read_text(encoding="utf-8"), "outside")
        self.assertEqual(sorted(path.name for path in self.root.iterdir()),
                         ["destination.sql", "existing.sql", "symlink.sql"])

    def test_partial_write_leaves_no_published_or_staged_file(self) -> None:
        from private_artifact_output import private_text_output

        target = self.root / "partial.sql"
        with self.assertRaises(RuntimeError):
            with private_text_output(target) as out:
                out.write("partial synthetic content")
                raise RuntimeError("injected failure")
        self.assertFalse(target.exists())
        self.assertEqual(list(self.root.iterdir()), [])


class ImporterCliTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.output = self.root / "output"
        self.output.mkdir(mode=0o700)

    def equipment_workbook(self, *, equipment_no: str = "ABC12-0001") -> Path:
        from import_equipment_master_list import REQUIRED_HEADERS

        workbook = Workbook()
        sheet = workbook.active
        sheet.title = "Sheet2"
        sheet.append(REQUIRED_HEADERS)
        row = {
            "장비 No": equipment_no,
            "사업장": "Synthetic Site",
            "계약처": "Synthetic Customer",
            "상태": "예비",
            "규격": "Synthetic Spec",
            "인양능력(톤)": "1T",
        }
        sheet.append([row.get(header) for header in REQUIRED_HEADERS])
        source = self.root / "synthetic-equipment.xlsx"
        workbook.save(source)
        return source

    def equipment_args(self, source: Path, output: Path) -> tuple[str, ...]:
        return (
            "--workbook", str(source), "--output", str(output),
            "--mode", "dry-run", "--target-branch-id",
            str(uuid.UUID(int=100)), "--expected-count", "1",
        )

    def coss_source(self) -> Path:
        source = self.root / "synthetic-coss"
        folder = source / "2026" / "5월" / "코스"
        folder.mkdir(parents=True, mode=0o700)
        workbook = Workbook()
        sheet = workbook.active
        sheet.title = "코스"
        sheet.append(["성명", "사번", "입사일"])
        sheet.append(["Synthetic Worker", "SYN-001", "2026-05-01"])
        workbook.save(folder / "synthetic-roster.xlsx")
        return source

    def coss_args(self, source: Path, summary: Path, sql: Path) -> tuple[str, ...]:
        args = [
            "--source", str(source), "--pay-period-start", "2026-05-01",
            "--pay-period-end", "2026-05-31", "--summary-out", str(summary),
            "--sql-out", str(sql),
        ]
        for index, slug in enumerate(ORG_SLUGS, start=1):
            args.extend(("--org-id", f"{slug}={uuid.UUID(int=index)}"))
        return tuple(args)

    def test_equipment_success_is_private_and_keeps_sql_and_cli_shapes(self) -> None:
        source = self.equipment_workbook()
        output = self.output / "equipment.sql"
        result = run_cli(EQUIPMENT, *self.equipment_args(source, output))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(mode(output), 0o600)
        self.assertTrue(output.read_text(encoding="utf-8").startswith("\\set ON_ERROR_STOP on"))
        self.assertIn("ROLLBACK;", output.read_text(encoding="utf-8"))
        try:
            summary = json.loads(result.stdout)
        except ValueError:
            self.fail("equipment CLI did not emit its documented JSON summary")
        self.assertEqual(summary["rows"], 1)
        self.assertEqual(summary["mode"], "dry-run")
        self.assertFalse("output" in summary)
        self.assertFalse("workbook" in summary)
        self.assertFalse(str(source) in result.stdout or str(source) in result.stderr)
        self.assertFalse(str(output) in result.stdout or str(output) in result.stderr)

    def test_equipment_rejects_existing_destination_without_altering_it(self) -> None:
        source = self.equipment_workbook()
        output = self.output / "existing.sql"
        output.write_text("existing synthetic artifact", encoding="utf-8")
        result = run_cli(EQUIPMENT, *self.equipment_args(source, output))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output.read_text(encoding="utf-8"), "existing synthetic artifact")

    def test_equipment_validation_and_argument_errors_do_not_echo_values(self) -> None:
        source = self.equipment_workbook(equipment_no=SENTINEL)
        output = self.output / "invalid.sql"
        result = run_cli(EQUIPMENT, *self.equipment_args(source, output))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(SENTINEL in result.stdout or SENTINEL in result.stderr)
        self.assertFalse(output.exists())

        valid_source = self.equipment_workbook()
        result = run_cli(EQUIPMENT, *self.equipment_args(valid_source, output), f"--unknown-{SENTINEL}")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(SENTINEL in result.stdout or SENTINEL in result.stderr)

    def test_coss_success_protects_both_outputs_and_keeps_shapes(self) -> None:
        source = self.coss_source()
        summary = self.output / "summary.json"
        sql = self.output / "coss.sql"
        result = run_cli(COSS, *self.coss_args(source, summary, sql))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(mode(summary), 0o600)
        self.assertEqual(mode(sql), 0o600)
        try:
            parsed = json.loads(summary.read_text(encoding="utf-8"))
            cli = json.loads(result.stdout)
        except ValueError:
            self.fail("COSS CLI did not emit documented JSON artifacts")
        self.assertEqual(parsed["files"], 1)
        self.assertEqual(cli["files"], 1)
        self.assertFalse("summary_path" in cli)
        self.assertFalse("sql_path" in cli)
        self.assertTrue(parsed["source_root"] == str(source))
        self.assertFalse(str(source) in result.stdout or str(source) in result.stderr)
        self.assertFalse(str(summary) in result.stdout or str(summary) in result.stderr)
        self.assertFalse(str(sql) in result.stdout or str(sql) in result.stderr)
        self.assertTrue(sql.read_text(encoding="utf-8").startswith(
            "-- Generated by scripts/import_coss_group_workbooks.py.\n"))
        self.assertTrue(sql.read_text(encoding="utf-8").endswith("COMMIT;\n"))

    def test_coss_rejects_existing_output_without_overwriting_it(self) -> None:
        source = self.coss_source()
        sql = self.output / "existing.sql"
        sql.write_text("existing synthetic artifact", encoding="utf-8")
        summary = self.output / "new-summary.json"
        result = run_cli(COSS, *self.coss_args(source, summary, sql))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sql.read_text(encoding="utf-8"), "existing synthetic artifact")
        self.assertFalse(summary.exists())

    def test_coss_argument_errors_do_not_echo_private_values(self) -> None:
        valid_source = self.coss_source()
        result = run_cli(COSS, *self.coss_args(valid_source, self.output / "unknown.json", self.output / "unknown.sql"), f"--unknown-{SENTINEL}")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(SENTINEL in result.stdout or SENTINEL in result.stderr)

        source = self.root / SENTINEL
        result = run_cli(COSS, *self.coss_args(source, self.output / "a.json", self.output / "b.sql"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(SENTINEL in result.stdout or SENTINEL in result.stderr)


if __name__ == "__main__":
    unittest.main()
