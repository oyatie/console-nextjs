#!/usr/bin/env python3
"""Derive Excel-to-browser parity expectations from the HR/org workbook.

No app code, no DB, no COSS state: this profiles only the source workbook and a
small CP949 CSV sample used by browser/E2E import checks.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import openpyxl

DEFAULT_WORKBOOK = Path("/Users/jasonlee/Downloads/Untitled spreadsheet.xlsx")


def text(value: Any) -> str:
    return str(value).strip() if value is not None else ""


def derive(workbook: Path) -> dict[str, Any]:
    wb = openpyxl.load_workbook(workbook, data_only=False, read_only=True)
    companies: list[dict[str, Any]] = []

    for ws in wb.worksheets:
        template_rows: list[dict[str, Any]] = []
        employee_rows: list[dict[str, Any]] = []
        for row_index, row in enumerate(ws.iter_rows(min_row=2, values_only=True), start=2):
            no = row[0] if len(row) > 0 else None
            source_company = text(row[1] if len(row) > 1 else None)
            name = text(row[3] if len(row) > 3 else None)
            record = {
                "source_row": row_index,
                "no": no,
                "source_company": source_company,
                "name": name,
            }
            if source_company or name:
                template_rows.append(record)
            if name:
                employee_rows.append(record)

        first_named = employee_rows[0] if employee_rows else None
        last_named = employee_rows[-1] if employee_rows else None
        companies.append(
            {
                "sheet": ws.title,
                "company": ws.title,
                "expected_employee_count": len(employee_rows),
                "expected_named_count": len(employee_rows),
                "source_template_row_count": len(template_rows),
                "first_name": first_named["name"] if first_named else None,
                "first_name_source_row": first_named["source_row"] if first_named else None,
                "last_name": last_named["name"] if last_named else None,
                "last_name_source_row": last_named["source_row"] if last_named else None,
            }
        )

    return {
        "source": str(workbook),
        "source_sha256": hashlib.sha256(workbook.read_bytes()).hexdigest(),
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "row_rule": "browser employee rows are workbook rows below the header with a nonblank 성명/name cell; blank-name template/staging rows are preserved in raw import/staging evidence but are not displayed as employees",
        "companies": companies,
    }


def write_cp949_fixture(path: Path, expectations: dict[str, Any]) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    rows = [["회사명", "부서명", "이름"]]
    for company in expectations["companies"]:
        rows.append([company["company"], "본사", company["first_name"] or ""])
        if company["last_name"] and company["last_name"] != company["first_name"]:
            rows.append([company["company"], "본사", company["last_name"]])

    # newline="" keeps csv's CRLF contract intact before CP949 encoding.
    text_buffer = []
    class Sink:
        def write(self, value: str) -> None:
            text_buffer.append(value)

    writer = csv.writer(Sink(), lineterminator="\r\n")
    writer.writerows(rows)
    csv_text = "".join(text_buffer)
    encoded = csv_text.encode("cp949")
    path.write_bytes(encoded)

    decoded = encoded.decode("euc-kr")
    if "�" in decoded:
        raise ValueError("decoded CP949 fixture contains replacement characters")
    decoded_rows = list(csv.reader(decoded.splitlines()))
    return {
        "path": str(path),
        "encoding_written": "cp949",
        "encoding_read_label": "euc-kr",
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "byte_count": len(encoded),
        "row_count_including_header": len(decoded_rows),
        "headers": decoded_rows[0],
        "replacement_character_count": decoded.count("�"),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workbook", type=Path, default=DEFAULT_WORKBOOK)
    parser.add_argument("--output", type=Path, help="write expectations JSON here")
    parser.add_argument("--cp949-csv", type=Path, help="write a CP949 org CSV evidence fixture here")
    args = parser.parse_args()

    if not args.workbook.exists():
        parser.error(f"workbook not found: {args.workbook}")

    expectations = derive(args.workbook)
    if args.cp949_csv:
        expectations["cp949_csv_evidence"] = write_cp949_fixture(args.cp949_csv, expectations)

    body = json.dumps(expectations, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(body, encoding="utf-8")
    else:
        sys.stdout.write(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
