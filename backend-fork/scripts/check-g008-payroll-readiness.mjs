import { createTextGate } from "./lib/text-gate.mjs";
import { beginGate, emitProvenanceIfRequested } from "./lib/gate-inputs.mjs";

beginGate({
  gate: "check:g008-payroll-readiness",
  script: "scripts/check-g008-payroll-readiness.mjs",
  documentInputs: [
    "docs/specs/hr-payroll-readiness.md",
  ],
});

const { requireIncludes, requireMatches, requireNotIncludes, reportGate } = createTextGate();

requireIncludes(
  "docs/specs/hr-payroll-readiness.md",
  "Payroll calculation remains blocked until the release gate is professionally validated",
  "payroll readiness spec keeps regulated calculation blocked",
);
requireIncludes(
  "docs/specs/hr-payroll-readiness.md",
  "annual-leave usage-promotion workflow",
  "annual leave usage-promotion workflow is modeled",
);
requireIncludes(
  "docs/specs/hr-payroll-readiness.md",
  "messenger/mail/workflow notification is a workflow object",
  "future notification integration routes through workflow/comms objects",
);

requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "CREATE TABLE payroll_draft_runs",
  "payroll draft run table exists",
);
requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "CREATE TABLE payroll_draft_lines",
  "payroll draft line table exists",
);
requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "CREATE TABLE annual_leave_obligations",
  "annual leave obligation table exists",
);
requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "BLOCKED_LEGAL_GATE",
  "draft lines fail closed behind legal gate",
);
requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "FORCE ROW LEVEL SECURITY",
  "payroll readiness tables force RLS",
);
requireIncludes(
  "backend/crates/platform/db/migrations/0074_create_payroll_readiness.sql",
  "GRANT SELECT, INSERT, UPDATE ON payroll_draft_runs TO console_rt",
  "runtime grants are explicit for draft runs",
);

requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "COSS Group 2026-05 live import",
  "live import source label is explicit",
);
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "data_import_rows",
  "stage SQL derives from governed import ledger",
);
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "Payroll calculation remains blocked until an official NTS row and professional validation are attached",
  "stage SQL does not enable payable payroll",
);
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "annual_leave_obligations",
  "stage SQL creates annual leave review obligations",
);
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "data_import.payroll_readiness_stage",
  "stage SQL audits the live derivation",
);
// The PROPERTY this pins is classification by an allowlisted header set, never a
// wildcard over whatever columns a workbook happens to carry. It used to be spelled
// as the `raw_row ?| array[...]` idiom, which also — accidentally — pinned KEY
// PRESENCE as the test. The allowlist is unchanged; how it is applied is not, so this
// now matches the allowlist itself rather than the operator that consumed it.
requireMatches(
  "scripts/stage_coss_group_payroll_readiness.sql",
  /kv\.key = ANY \(array\[/,
  "stage SQL classifies payroll/attendance source rows by allowlisted headers",
);
requireNotIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "SELECT *",
  "stage SQL avoids broad raw selects",
);

// Provenance of the material that reaches payroll_draft_lines. `data_import_runs.status`
// admits PREVIEWED/DRY_RUN/APPLIED/FAILED and `data_import_rows.row_status` admits
// CANDIDATE/PRESERVED/ERROR (migration 0070). The stage SQL filtered on NEITHER: the only
// thing keeping unapplied rows out of a payroll roster was a human hand-typing one vetted
// source_filename. Measured against a live PostgreSQL: with these two predicates removed, a
// never-applied DRY_RUN and a run whose rows are all ERROR each materialise a roster line.
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "run.status = 'APPLIED'",
  "stage SQL admits only APPLIED import runs as payroll source material",
);
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "r.row_status <> 'ERROR'",
  "stage SQL never treats an ERROR import row as payroll source material",
);
// Re-running the stage must not rewind a run that has left the pre-close states. Without the
// guard the ON CONFLICT DO UPDATE reset status to BLOCKED_LEGAL_GATE unconditionally, so a
// re-stage over a PAID run rewound it -- measured: PAID -> BLOCKED_LEGAL_GATE.
requireIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "WHERE payroll_draft_runs.status IN ('STAGED', 'BLOCKED_LEGAL_GATE', 'READY_FOR_REVIEW')",
  "stage SQL refuses to rewind a run past the pre-close states",
);

// Source material must be a non-blank VALUE, not merely a present KEY. `?|` asks
// whether the spreadsheet HAS the column; measured against a live PostgreSQL, a row
// whose 출근 cell was empty produced attendance_source_row_count = 1 exactly as if it
// were filled, so the close preflight's 근태 원천 확보 check passed on a blank column.
requireNotIncludes(
  "scripts/stage_coss_group_payroll_readiness.sql",
  "?|array",
  "stage SQL never treats a merely PRESENT column as source material",
);
// Counted, not merely present. There are five source-material flags
// (is_payroll_source, is_attendance_source, is_leave_source, has_gross_pay_source,
// has_net_pay_source); a plain "includes" check passes while four of the five are
// weakened, which is exactly the hole this gate exists to refuse.
requireMatches(
  "scripts/stage_coss_group_payroll_readiness.sql",
  /(?:btrim\(kv\.value\) <> ''[\s\S]*?){5}/,
  "all five stage SQL source-material flags require a non-blank value",
);

// THE PRODUCTION WRITER, not only the script.
//
// Until PR #846 `payroll_draft_lines` had no production writer, so pinning the
// staging SQL pinned the only path a roster could come from. It is not any more:
// `roster::materialise_roster_in_tx` runs on every `payroll.create_run`, and the
// twelve pins above sit on a file production no longer needs. Left alone, the two
// encodings of "what may become a payroll roster" could drift, and the gate would
// stay green while the executed one weakened.
//
// These pin the SAME provenance properties on the writer that actually runs.
// Deliberately NOT re-pointed from the script verbatim: the script's
// `raw_row ?| array[...]` idiom is the key-presence fabrication vector, and a
// gate that REQUIRED it on the writer would mandate the bug.
const rosterWriter = "backend/crates/payroll/adapter-postgres/src/roster.rs";
requireIncludes(
  rosterWriter,
  "run.status = 'APPLIED'",
  "production roster writer admits only APPLIED import runs",
);
requireIncludes(
  rosterWriter,
  "r.row_status <> 'ERROR'",
  "production roster writer never treats an ERROR import row as material",
);
requireIncludes(
  rosterWriter,
  "run.pay_period_start = $3",
  "production roster writer scopes by the declared pay period, by equality",
);
// Counted, not merely present: there are four source-material flags, and a plain
// `includes` passes while three of them are weakened.
requireMatches(
  rosterWriter,
  /(?:btrim\(kv\.value\) <> ''[\s\S]*?){4}/,
  "all four production source-material flags require a non-blank value",
);
requireNotIncludes(
  rosterWriter,
  "?|array",
  "production roster writer never treats a merely PRESENT column as material",
);
// 0222 revoked DELETE on this table from console_rt and asserts the revocation,
// so a reconciliation delete raises 42501 at PLAN time — killing every
// payroll.create_run, not just the re-stage that introduced it.
requireNotIncludes(
  rosterWriter,
  "DELETE FROM payroll_draft_lines",
  "production roster writer never deletes: console_rt holds no DELETE on this table",
);

requireIncludes(
  "package.json",
  '"check:g008-payroll-readiness"',
  "package script is wired",
);
requireIncludes(
  ".github/workflows/ci.yml",
  "npm run check:g008-payroll-readiness",
  "CI runs G008 payroll readiness gate",
);

emitProvenanceIfRequested();
reportGate("G008 payroll readiness gate passed");
