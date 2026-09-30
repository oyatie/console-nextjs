#!/usr/bin/env node
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { createEvidenceRecord, assertEvidenceSafe } from "./evidence-ledger.mjs";
import { prepareIntent, sha256 } from "./adapter-sdk.mjs";

export const NHIS_LOSS_REPORT_SOURCE_URLS = [
  "https://www.nhis.or.kr/static/html/wbdb/f/wbdbf0201.html",
  "https://edi.nhis.or.kr/webedi/file_sy/all_sangsil.html",
];

export const NHIS_LOSS_REPORT_COLUMNS = [
  { key: "workplace_management_number", header: "사업장관리번호" },
  { key: "employee_name", header: "성명" },
  { key: "resident_registration_number_token", header: "주민등록번호(대체토큰)" },
  { key: "loss_date", header: "상실일" },
  { key: "loss_reason_code", header: "상실부호" },
  { key: "national_pension", header: "국민연금" },
  { key: "health_insurance", header: "건강보험" },
  { key: "employment_insurance", header: "고용보험" },
  { key: "industrial_accident_insurance", header: "산재보험" },
  { key: "annual_income_amount", header: "보수총액(원)" },
  { key: "review_note", header: "검토메모" },
];

function csvEscape(value) {
  const text = String(value ?? "");
  if (/[",\n\r]/.test(text)) {
    return `"${text.replaceAll('"', '""')}"`;
  }
  return text;
}

export function validateLossReportRows(rows) {
  if (!Array.isArray(rows) || rows.length === 0) {
    throw new Error("loss report rows are required");
  }
  if (rows.length > 500) {
    throw new Error("NHIS EDI loss report fixture exceeds 500-row official file limit");
  }
  for (const [index, row] of rows.entries()) {
    for (const column of NHIS_LOSS_REPORT_COLUMNS) {
      if (!(column.key in row)) {
        throw new Error(`row ${index + 1} missing ${column.key}`);
      }
    }
    if (!/^\d{4}-\d{2}-\d{2}$/.test(row.loss_date)) {
      throw new Error(`row ${index + 1} loss_date must be YYYY-MM-DD`);
    }
    if (!/^RRN_FIXTURE_[A-Z0-9_]+$/.test(row.resident_registration_number_token)) {
      throw new Error(`row ${index + 1} must use resident_registration_number_token placeholder, not a real RRN`);
    }
    const serialized = JSON.stringify(row);
    if (/\b\d{6}-?[1-4]\d{6}\b/.test(serialized)) {
      throw new Error(`row ${index + 1} contains unredacted resident registration number pattern`);
    }
  }
  return rows;
}

export function renderLossReportCsv(rows) {
  validateLossReportRows(rows);
  const header = NHIS_LOSS_REPORT_COLUMNS.map((column) => csvEscape(column.header)).join(",");
  const body = rows.map((row) => NHIS_LOSS_REPORT_COLUMNS.map((column) => csvEscape(row[column.key])).join(","));
  return `\ufeff${[header, ...body].join("\n")}\n`;
}

export function assertNoLiveSubmission() {
  throw new Error("NHIS EDI live submission is prohibited in this fixture-only POC; use manual upload runbook after human review.");
}

export async function generateNhisLossReportFixture({ rows, outputPath = ".tmp/nhis-edi-loss-report.fixture.csv" }) {
  validateLossReportRows(rows);
  const csv = renderLossReportCsv(rows);
  const intent = prepareIntent({
    connector_id: "nhis_edi_loss_report",
    workflow_id: "social_insurance.loss_report.generate_file",
    side_effect_class: "generated_file",
    payload: { row_count: rows.length, output_path: outputPath },
  });
  await mkdir(dirname(resolve(outputPath)), { recursive: true });
  await writeFile(resolve(outputPath), csv, "utf8");
  const evidence = createEvidenceRecord({
    connectorId: "nhis_edi_loss_report",
    workflowId: "social_insurance.loss_report.generate_file",
    intentHash: intent.intent_hash,
    parserVersion: "nhis-edi-loss-report-fixture-v1",
    sourceUrls: NHIS_LOSS_REPORT_SOURCE_URLS,
    transcript: `Generated fixture NHIS EDI loss-report file for ${rows.length} rows; no live filing; manual upload runbook required`,
    output: {
      output_path: outputPath,
      row_count: rows.length,
      file_sha256: sha256(csv),
      source_urls: NHIS_LOSS_REPORT_SOURCE_URLS,
      live_submission: false,
    },
    observedAt: "fixture-time",
  });
  assertEvidenceSafe(evidence);
  return {
    workflow_id: "social_insurance.loss_report.generate_file",
    execution_mode: "fixture_only",
    side_effect_class: "generated_file",
    output_path: outputPath,
    row_count: rows.length,
    csv_sha256: sha256(csv),
    evidence_record: evidence,
  };
}

export async function loadFixtureRows(fixturePath = "docs/benchmarks/fixtures/korean-connectivity/nhis-edi-loss-report.fixture.json") {
  const fixture = JSON.parse(await readFile(resolve(fixturePath), "utf8"));
  return validateLossReportRows(fixture.rows);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const fixtureArgIndex = process.argv.indexOf("--fixture");
  const outArgIndex = process.argv.indexOf("--out");
  const fixturePath = fixtureArgIndex === -1 ? undefined : process.argv[fixtureArgIndex + 1];
  const outputPath = outArgIndex === -1 ? ".tmp/nhis-edi-loss-report.fixture.csv" : process.argv[outArgIndex + 1];
  const rows = await loadFixtureRows(fixturePath);
  const result = await generateNhisLossReportFixture({ rows, outputPath });
  console.log(JSON.stringify({ ok: true, ...result }, null, 2));
}
